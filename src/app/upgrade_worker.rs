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
use crate::hal::{NodeId, UsbRoute};
use crate::streaming_data_service::data_transfer::DataTransfer;
use crate::utils::WriteMonitor;
use anyhow::bail;
use crc::{Crc, CRC_64_REDIS};
use humansize::{format_size, DECIMAL};
use std::io::{Error, ErrorKind};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncSeekExt;
use tokio::io::BufStream;
use tokio::io::{sink, AsyncRead};
use tokio::sync::watch;
use tokio::task::spawn_blocking;
use tokio::{
    fs,
    io::{self, AsyncWrite, AsyncWriteExt},
};
use tokio_util::sync::CancellationToken;

const TMP_UPGRADE_DIR: &str = "/tmp/os_upgrade";
const BLOCK_WRITE_SIZE: usize = BLOCK_READ_SIZE; // 512Kib
const BLOCK_READ_SIZE: usize = 524288; // 512Kib

// Contains collection of functions that execute some business flow in relation
// to file transfers in the BMC. See `flash_node` and `os_update`.
pub struct UpgradeWorker {
    do_crc_validation: bool,
    data_transfer: DataTransfer,
    cancel: CancellationToken,
    written_sender: watch::Sender<u64>,
}

impl UpgradeWorker {
    pub fn new(
        do_crc_validation: bool,
        data_transfer: DataTransfer,
        cancel: CancellationToken,
        written_sender: watch::Sender<u64>,
    ) -> Self {
        Self {
            do_crc_validation,
            data_transfer,
            cancel,
            written_sender,
        }
    }

    /// Logic to program a given OS image to a node. Uses a [`DataTransfer`]
    /// abstraction as source of the image data. The transfer can be interrupted
    /// at any time when the `CancellationToken` is cancelled. When a transfer
    /// is interrupted or failed, it will always powers off the Node and
    /// restores the USB mode equally to a successful flow would.
    pub async fn flash_node(
        mut self,
        bmc: Arc<BmcApplication>,
        node: NodeId,
    ) -> anyhow::Result<()> {
        let device = bmc.node_in_flash(node, UsbRoute::Bmc).await?;

        let result = async move {
            let reader = self.data_transfer.reader().await?;
            let mut buf_stream =
                BufStream::with_capacity(BLOCK_READ_SIZE, BLOCK_WRITE_SIZE, device);
            let (bytes_written, written_crc) =
                self.try_write_node(node, reader, &mut buf_stream).await?;

            if self.do_crc_validation {
                buf_stream.seek(std::io::SeekFrom::Start(0)).await?;
                flush_file_caches().await?;
                self.try_validate_crc(node, written_crc, buf_stream.take(bytes_written))
                    .await?;
            } else {
                log::info!("user skipped crc check");
            }

            Ok::<(), anyhow::Error>(())
        }
        .await;

        if let Ok(()) = result {
            log::info!("Flashing {node} successful, restoring USB & power settings.");
        }

        // disregarding the result, set the BMC in the finalized state.
        bmc.activate_slot(node.to_inverse_bitfield(), node.to_bitfield())
            .await?;
        bmc.usb_boot(node, false).await?;
        bmc.configure_usb(bmc.get_usb_mode().await).await?;
        result
    }

    async fn try_write_node(
        &mut self,
        node: NodeId,
        source_reader: impl AsyncRead + 'static + Unpin,
        mut node_writer: &mut (impl AsyncWrite + 'static + Unpin),
    ) -> anyhow::Result<(u64, u64)> {
        log::info!("started writing to {node}");

        let crc = Crc::<u64>::new(&CRC_64_REDIS);
        let mut write_watcher = WriteMonitor::new(&mut node_writer, &mut self.written_sender, &crc);

        let bytes_written = copy_or_cancel(source_reader, &mut write_watcher, &self.cancel).await?;
        let crc = write_watcher.crc();

        log::info!(
            "Wrote {}, crc: {}",
            format_size(bytes_written, DECIMAL),
            crc
        );

        Ok((bytes_written, crc))
    }

    async fn try_validate_crc(
        &mut self,
        node: NodeId,
        expected_crc: u64,
        node_reader: impl AsyncRead + 'static + Unpin,
    ) -> anyhow::Result<()> {
        log::info!("Verifying checksum of data on node {node}");

        let crc = Crc::<u64>::new(&CRC_64_REDIS);
        let mut sink = WriteMonitor::new(sink(), &mut self.written_sender, &crc);
        copy_or_cancel(node_reader, &mut sink, &self.cancel).await?;
        let dev_checksum = sink.crc();

        if expected_crc != dev_checksum {
            bail!(
                "crc error. expected {}, calculated {}",
                expected_crc.to_string(),
                dev_checksum.to_string()
            );
        }

        Ok(())
    }

    pub async fn os_update(mut self) -> anyhow::Result<()> {
        let file_name = self.data_transfer.file_name()?.to_owned();
        let source = self.data_transfer.reader().await?;
        log::info!("start firmware upgrade {}", file_name.to_string_lossy());

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
        copy_or_cancel(source, &mut writer, &self.cancel).await?;

        let result = spawn_blocking(move || {
            Command::new("sh")
                .arg("-c")
                .arg(&format!("osupdate {}", os_update_img.to_string_lossy()))
                .status()
        })
        .await?;

        tokio::fs::remove_dir_all(TMP_UPGRADE_DIR).await?;

        let success = result?;
        if !success.success() {
            bail!("failed firmware upgrade ({})", success);
        }

        Ok(())
    }
}

/// Copies bytes from `reader` to `writer` until the reader is exhausted. This function
/// returns an `io::Error(Interrupted)` in case a cancel was issued.
async fn copy_or_cancel<L, W>(
    mut reader: L,
    mut writer: &mut W,
    cancel: &CancellationToken,
) -> std::io::Result<u64>
where
    L: AsyncRead + std::marker::Unpin,
    W: AsyncWrite + std::marker::Unpin,
{
    let copy_task = tokio::io::copy(&mut reader, &mut writer);
    let cancel = cancel.cancelled();

    let bytes_copied: u64;
    tokio::select! {
        res = copy_task =>  bytes_copied = res?,
        _ = cancel => return Err(Error::from(ErrorKind::Interrupted)),
    };

    log::debug!("copied {} bytes", bytes_copied);
    writer.flush().await?;
    Ok(bytes_copied)
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
        let crc = Crc::<u64>::new(&CRC_64_REDIS);
        let buffer = random_array::<{ 10024 * 1024 }>();
        let expected_crc = crc.checksum(&buffer);

        let mut buf_writer = BufWriter::new(Vec::new());
        let cursor = std::io::Cursor::new(&buffer);

        let (mut sender, mut receiver) = watch::channel(0u64);
        let mut write_watcher = WriteMonitor::new(&mut buf_writer, &mut sender, &crc);
        copy_or_cancel(cursor, &mut write_watcher, &CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(expected_crc, write_watcher.crc());
        assert_eq!(&buffer, buf_writer.get_ref());
        assert_eq!(*receiver.borrow_and_update(), buffer.len() as u64);
    }
}
