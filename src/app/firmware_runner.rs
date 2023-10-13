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
use super::bmc_application::UsbConfig;
use crate::api::streaming_data_service::ReaderContext;
use crate::app::bmc_application::BmcApplication;
use crate::utils::{logging_sink, reader_with_crc64, WriteWatcher};
use crate::{
    firmware_update::{FlashProgress, FlashStatus, FlashingError, SUPPORTED_DEVICES},
    hal::{NodeId, UsbRoute},
};
use anyhow::bail;
use crc::Crc;
use crc::CRC_64_REDIS;
use std::cmp::Ordering;
use std::io::{Error, ErrorKind};
use std::path::PathBuf;
use std::process::Command;
use std::{sync::Arc, time::Duration};
use tokio::fs::OpenOptions;
use tokio::io::sink;
use tokio::io::AsyncReadExt;
use tokio::{
    fs,
    io::{self, AsyncRead, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc::{channel, Sender},
    time::sleep,
};
use tokio_util::sync::CancellationToken;

const REBOOT_DELAY: Duration = Duration::from_millis(500);
const MOUNT_POINT: &str = "/tmp/os_upgrade";

// Contains collection of functions that execute some business flow in relation
// to file transfers in the BMC. See `flash_node` and `os_update`.
pub struct FirmwareRunner {
    pub filename: String,
    pub context: Option<ReaderContext>,
    pub progress_sender: Sender<FlashProgress>,
}

impl FirmwareRunner {
    pub fn new(filename: PathBuf, reader_context: ReaderContext) -> Self {
        let (progress_sender, progress_receiver) = channel(32);
        logging_sink(progress_receiver);

        Self {
            filename: filename
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or(filename.to_string_lossy().to_string()),
            progress_sender,
            context: Some(reader_context),
        }
    }

    pub async fn flash_node(
        mut self,
        bmc: Arc<BmcApplication>,
        node: NodeId,
    ) -> anyhow::Result<()> {
        let mut device = bmc
            .configure_node_for_fwupgrade(
                node,
                UsbRoute::Bmc,
                self.progress_sender.clone(),
                SUPPORTED_DEVICES.keys(),
            )
            .await?;

        let mut progress_state = FlashProgress {
            message: String::new(),
            status: FlashStatus::Setup,
        };

        progress_state.message = format!("Writing {:?}", self.filename);
        self.progress_sender.send(progress_state.clone()).await?;

        let context = self.context.take().expect("context should always be set");
        let write_watcher = WriteWatcher::new(&mut device, context.written_sender);
        let img_checksum =
            copy_with_crc(context.reader, write_watcher, context.size, &context.cancel).await?;

        progress_state.message = String::from("Verifying checksum...");
        self.progress_sender.send(progress_state.clone()).await?;

        device.seek(std::io::SeekFrom::Start(0)).await?;
        flush_file_caches().await?;

        let dev_checksum =
            copy_with_crc(&mut device, sink(), context.size, &context.cancel).await?;

        if img_checksum != dev_checksum {
            self.progress_sender
                .send(FlashProgress {
                    status: FlashStatus::Error(FlashingError::ChecksumMismatch),
                    message: format!(
                        "Source and destination checksum mismatch: {:#x} != {:#x}",
                        img_checksum, dev_checksum
                    ),
                })
                .await?;

            bail!(FlashingError::ChecksumMismatch)
        }

        progress_state.message = String::from("Flashing successful, restarting device...");
        self.progress_sender.send(progress_state.clone()).await?;

        bmc.activate_slot(!node.to_bitfield(), node.to_bitfield())
            .await?;

        //TODO: we probably want to restore the state prior flashing
        bmc.usb_boot(node, false).await?;
        bmc.configure_usb(UsbConfig::UsbA(node)).await?;

        sleep(REBOOT_DELAY).await;

        bmc.activate_slot(node.to_bitfield(), node.to_bitfield())
            .await?;

        progress_state.message = String::from("Done");
        self.progress_sender.send(progress_state).await?;
        Ok(())
    }

    pub async fn os_update(mut self) -> anyhow::Result<()> {
        log::info!("start os update");

        let mut os_update_img = PathBuf::from(MOUNT_POINT);
        os_update_img.push(&self.filename);

        tokio::fs::create_dir_all(MOUNT_POINT).await?;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&os_update_img)
            .await?;

        let context = self.context.take().expect("context should always be set");
        let write_watcher = WriteWatcher::new(&mut file, context.written_sender);
        let result = copy_with_crc(context.reader, write_watcher, context.size, &context.cancel)
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

/// Copies `self.size` bytes from `reader` to `writer` and returns the crc
/// that was calculated over the reader. This function returns an
/// `io::Error(Interrupted)` in case a cancel was issued.
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
    let mut crc_reader = reader_with_crc64(reader.take(size), &crc);

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
            format!("missing {} bytes", total_size - len),
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
