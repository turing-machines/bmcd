// Copyright 2023 Turing Machines
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
use crate::app::bmc_application::BmcApplication;
use crate::firmware_update::FwUpdateError;
use crate::utils::{reader_with_crc64, WriteWatcher};
use crate::{
    firmware_update::SUPPORTED_DEVICES,
    hal::{NodeId, UsbRoute},
};
use anyhow::bail;
use crc::Crc;
use crc::CRC_64_REDIS;
use humansize::{format_size, DECIMAL};
use std::cmp::Ordering;
use std::io::{Error, ErrorKind};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::sink;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncSeekExt;
use tokio::sync::watch;
use tokio::{
    fs,
    io::{self, AsyncRead, AsyncWrite, AsyncWriteExt},
};
use tokio_util::sync::CancellationToken;

const MOUNT_POINT: &str = "/tmp/os_upgrade";

// Contains collection of functions that execute some business flow in relation
// to file transfers in the BMC. See `flash_node` and `os_update`.
pub struct FirmwareRunner {
    reader: Box<dyn AsyncRead + Send + Sync + Unpin>,
    file_name: String,
    size: u64,
    cancel: CancellationToken,
    written_sender: watch::Sender<u64>,
}

impl FirmwareRunner {
    pub fn new(
        reader: Box<dyn AsyncRead + Send + Sync + Unpin>,
        file_name: String,
        size: u64,
        cancel: CancellationToken,
        written_sender: watch::Sender<u64>,
    ) -> Self {
        Self {
            reader,
            file_name,
            size,
            cancel,
            written_sender,
        }
    }

    pub async fn flash_node(self, bmc: Arc<BmcApplication>, node: NodeId) -> anyhow::Result<()> {
        let mut device = bmc
            .configure_node_for_fwupgrade(node, UsbRoute::Bmc, SUPPORTED_DEVICES.keys())
            .await?;

        log::info!("started writing to {node}");
        let write_watcher = WriteWatcher::new(&mut device, self.written_sender);
        let img_checksum = copy_with_crc(
            self.reader.take(self.size),
            write_watcher,
            self.size,
            &self.cancel,
        )
        .await?;

        log::info!("Verifying checksum of written data to {node}");
        device.seek(std::io::SeekFrom::Start(0)).await?;
        flush_file_caches().await?;

        let dev_checksum =
            copy_with_crc(&mut device.take(self.size), sink(), self.size, &self.cancel).await?;

        if img_checksum != dev_checksum {
            log::error!(
                "Source and destination checksum mismatch: {:#x} != {:#x}",
                img_checksum,
                dev_checksum
            );

            bail!(FwUpdateError::ChecksumMismatch)
        }

        log::info!("Flashing {node} successful, restoring USB & power settings...");
        bmc.activate_slot(node.to_inverse_bitfield(), node.to_bitfield())
            .await?;
        bmc.usb_boot(node, false).await?;
        bmc.configure_usb(bmc.get_usb_mode().await).await
    }

    pub async fn os_update(self) -> anyhow::Result<()> {
        log::info!("start os update");

        let mut os_update_img = PathBuf::from(MOUNT_POINT);
        os_update_img.push(&self.file_name);

        tokio::fs::create_dir_all(MOUNT_POINT).await?;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&os_update_img)
            .await?;

        let write_watcher = WriteWatcher::new(&mut file, self.written_sender);
        let result = copy_with_crc(self.reader, write_watcher, self.size, &self.cancel)
            .await
            .and_then(|crc| {
                log::info!("crc os_update image: {}", crc);
                Command::new("sh")
                    .arg("-c")
                    .arg(&format!("osupdate {}", os_update_img.to_string_lossy()))
                    .status()
            });

        tokio::fs::remove_dir_all(MOUNT_POINT).await?;

        let success = result?;
        if !success.success() {
            bail!("failed os_update ({})", success);
        }

        Ok(())
    }
}

/// Copies bytes from `reader` to `writer` until the reader is exhausted. This
/// function returns the crc that was calculated over the reader. This function
/// returns an `io::Error(Interrupted)` in case a cancel was issued.
async fn copy_with_crc<L, W>(
    reader: L,
    mut writer: W,
    size: u64,
    cancel: &CancellationToken,
) -> std::io::Result<u64>
where
    L: AsyncRead + std::marker::Unpin,
    W: AsyncWrite + std::marker::Unpin,
{
    let crc = Crc::<u64>::new(&CRC_64_REDIS);
    let mut crc_reader = reader_with_crc64(reader, &crc);

    let copy_task = tokio::io::copy(&mut crc_reader, &mut writer);
    let cancel = cancel.cancelled();

    let bytes_copied;
    tokio::select! {
        res = copy_task =>  bytes_copied = res?,
        _ = cancel => return Err(Error::from(ErrorKind::Interrupted)),
    };

    validate_size(bytes_copied, size)?;

    writer.flush().await?;

    Ok(crc_reader.crc())
}

fn validate_size(len: u64, total_size: u64) -> std::io::Result<()> {
    match len.cmp(&total_size) {
        Ordering::Less => Err(Error::new(
            ErrorKind::UnexpectedEof,
            format!("missing {} bytes", format_size(total_size - len, DECIMAL)),
        )),
        Ordering::Greater => panic!("reads are capped to self.size"),
        Ordering::Equal => Ok(()),
    }
}

async fn flush_file_caches() -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open("/proc/sys/vm/drop_caches")
        .await?;

    // Free reclaimable slab objects and page cache
    file.write_u8(b'3').await
}

#[cfg(test)]
mod test {

    use super::*;
    use crc::CRC_64_REDIS;
    use rand::RngCore;
    use tokio::io::BufWriter;

    fn random_array<const SIZE: usize>() -> Vec<u8> {
        let mut array = vec![0; SIZE];
        rand::thread_rng().fill_bytes(&mut array);
        array
    }

    #[tokio::test]
    async fn crc_reader_test() {
        let buffer = random_array::<{ 10024 * 1024 }>();
        let expected_crc = Crc::<u64>::new(&CRC_64_REDIS).checksum(&buffer);

        let mut buf_writer = BufWriter::new(Vec::new());
        let cursor = std::io::Cursor::new(&buffer);

        assert_eq!(
            expected_crc,
            copy_with_crc(
                cursor,
                &mut buf_writer,
                buffer.len() as u64,
                &CancellationToken::new()
            )
            .await
            .unwrap()
        );
        assert_eq!(&buffer, buf_writer.get_ref());
    }
}
