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
use crate::streaming_data_service::data_transfer::DataTransfer;
use crate::utils::WriteMonitor;
use crate::{
    firmware_update::SUPPORTED_DEVICES,
    hal::{NodeId, UsbRoute},
};
use anyhow::bail;
use core::time::Duration;
use crc::{Crc, CRC_64_REDIS};
use humansize::{format_size, DECIMAL};
use std::cmp::Ordering;
use std::io::{Error, ErrorKind};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncSeekExt;
use tokio::io::BufStream;
use tokio::io::{sink, AsyncBufRead};
use tokio::sync::watch;
use tokio::time::sleep;
use tokio::{
    fs,
    io::{self, AsyncWrite, AsyncWriteExt},
};
use tokio_util::sync::CancellationToken;

const TMP_UPGRADE_DIR: &str = "/tmp/os_upgrade";
const BLOCK_WRITE_SIZE: usize = 4194304; // 4Mib
const BLOCK_READ_SIZE: usize = 524288; // 512Kib

// Contains collection of functions that execute some business flow in relation
// to file transfers in the BMC. See `flash_node` and `os_update`.
pub struct UpgradeWorker {
    data_transfer: DataTransfer,
    cancel: CancellationToken,
    written_sender: watch::Sender<u64>,
}

impl UpgradeWorker {
    pub fn new(
        data_transfer: DataTransfer,
        cancel: CancellationToken,
        written_sender: watch::Sender<u64>,
    ) -> Self {
        Self {
            data_transfer,
            cancel,
            written_sender,
        }
    }

    pub async fn flash_node(
        mut self,
        bmc: Arc<BmcApplication>,
        node: NodeId,
    ) -> anyhow::Result<()> {
        log::info!("Start OS install");
        let size = self.data_transfer.size()?;
        let source = self.data_transfer.buf_reader().await?;
        let device = bmc
            .configure_node_for_fwupgrade(node, UsbRoute::Bmc, SUPPORTED_DEVICES.keys())
            .await?;
        let mut buf_stream = BufStream::with_capacity(BLOCK_READ_SIZE, BLOCK_WRITE_SIZE, device);

        log::info!("started writing to {node}");
        let crc = Crc::<u64>::new(&CRC_64_REDIS);
        let mut write_watcher = WriteMonitor::new(&mut buf_stream, &mut self.written_sender, &crc);
        pedantic_buf_copy(source, &mut write_watcher, size, &self.cancel).await?;
        let img_checksum = write_watcher.crc();

        log::info!("Verifying checksum of written data to {node}");
        buf_stream.seek(std::io::SeekFrom::Start(0)).await?;
        flush_file_caches().await?;

        let (mut sender, receiver) = watch::channel(0u64);
        tokio::spawn(progress_printer(receiver));

        let mut write_watcher = WriteMonitor::new(sink(), &mut sender, &crc);
        pedantic_buf_copy(
            &mut buf_stream.take(size),
            &mut write_watcher,
            size,
            &self.cancel,
        )
        .await?;
        let dev_checksum = write_watcher.crc();

        if img_checksum != dev_checksum {
            log::error!(
                "Source and destination checksum mismatch: {:#x} != {:#x}",
                img_checksum,
                dev_checksum
            );

            bail!(FwUpdateError::ChecksumMismatch)
        }

        log::info!("Flashing {node} successful, restoring USB & power settings.");
        bmc.activate_slot(node.to_inverse_bitfield(), node.to_bitfield())
            .await?;
        bmc.usb_boot(node, false).await?;
        bmc.configure_usb(bmc.get_usb_mode().await).await
    }

    pub async fn os_update(mut self) -> anyhow::Result<()> {
        log::info!("start os update");
        let size = self.data_transfer.size()?;
        let file_name = self.data_transfer.file_name()?.to_owned();
        let source = self.data_transfer.buf_reader().await?;

        let mut os_update_img = PathBuf::from(TMP_UPGRADE_DIR);
        os_update_img.push(&file_name);

        tokio::fs::create_dir_all(TMP_UPGRADE_DIR).await?;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&os_update_img)
            .await?;

        let crc = Crc::<u64>::new(&CRC_64_REDIS);
        let mut writer = WriteMonitor::new(&mut file, &mut self.written_sender, &crc);
        let result = pedantic_buf_copy(source, &mut writer, size, &self.cancel)
            .await
            .and_then(|_| {
                log::info!("crc os_update image: {}.", writer.crc());
                Command::new("sh")
                    .arg("-c")
                    .arg(&format!("osupdate {}", os_update_img.to_string_lossy()))
                    .status()
            });

        tokio::fs::remove_dir_all(TMP_UPGRADE_DIR).await?;

        let success = result?;
        if !success.success() {
            bail!("failed os_update ({})", success);
        }

        Ok(())
    }
}

/// Copies bytes from `reader` to `writer` until the reader is exhausted. This function
/// returns an `io::Error(Interrupted)` in case a cancel was issued.
async fn pedantic_buf_copy<L, W>(
    mut reader: L,
    mut writer: &mut W,
    size: u64,
    cancel: &CancellationToken,
) -> std::io::Result<()>
where
    L: AsyncBufRead + std::marker::Unpin,
    W: AsyncWrite + std::marker::Unpin,
{
    let copy_task = tokio::io::copy_buf(&mut reader, &mut writer);
    let cancel = cancel.cancelled();

    let bytes_copied;
    tokio::select! {
        res = copy_task =>  bytes_copied = res?,
        _ = cancel => return Err(Error::from(ErrorKind::Interrupted)),
    };

    validate_size(bytes_copied, size)?;

    writer.flush().await?;
    Ok(())
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

async fn progress_printer(mut watcher: watch::Receiver<u64>) {
    loop {
        sleep(Duration::from_secs(5)).await;
        if let Err(_) = watcher.changed().await {
            return;
        }
        log::info!(
            "read {}",
            humansize::format_size(*watcher.borrow_and_update(), DECIMAL)
        );
    }
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
        let crc = Crc::<u64>::new(&CRC_64_REDIS);
        let buffer = random_array::<{ 10024 * 1024 }>();
        let expected_crc = crc.checksum(&buffer);

        let mut buf_writer = BufWriter::new(Vec::new());
        let cursor = std::io::Cursor::new(&buffer);

        let (mut sender, mut receiver) = watch::channel(0u64);
        let mut write_watcher = WriteMonitor::new(&mut buf_writer, &mut sender, &crc);
        pedantic_buf_copy(
            cursor,
            &mut write_watcher,
            buffer.len() as u64,
            &CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(expected_crc, write_watcher.crc());
        assert_eq!(&buffer, buf_writer.get_ref());
        assert_eq!(*receiver.borrow_and_update(), buffer.len() as u64);
    }
}
