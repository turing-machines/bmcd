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
use crate::app::bmc_application::BmcApplication;
use crate::middleware::{
    firmware_update::{FlashProgress, FlashStatus, FlashingError, SUPPORTED_DEVICES},
    NodeId, UsbRoute,
};
use crate::utils::logging_sink;
use anyhow::{bail, ensure, Context};
use crc::{Crc, CRC_64_REDIS};
use futures::TryFutureExt;
use std::{sync::Arc, time::Duration};
use tokio::{
    fs,
    io::{self, AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc::{channel, Receiver, Sender},
    time::{sleep, Instant},
};
use tokio_util::sync::CancellationToken;

const REBOOT_DELAY: Duration = Duration::from_millis(500);
const BUF_SIZE: u64 = 8 * 1024;
const PROGRESS_REPORT_PERCENT: u64 = 5;

pub struct FirmwareRunner<R: AsyncRead> {
    pub filename: String,
    pub size: u64,
    pub byte_stream: R,
    pub progress_sender: Sender<FlashProgress>,
    pub cancel: CancellationToken,
}

impl<R: AsyncRead + Unpin> FirmwareRunner<R> {
    pub fn new(filename: String, size: u64, byte_stream: R, cancel: CancellationToken) -> Self {
        let (progress_sender, progress_receiver) = channel(32);
        logging_sink(progress_receiver);

        Self {
            filename,
            size,
            byte_stream,
            progress_sender,
            cancel,
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

        let img_checksum = self.write_to_device(&mut device).await?;

        progress_state.message = String::from("Verifying checksum...");
        self.progress_sender.send(progress_state.clone()).await?;

        device.seek(std::io::SeekFrom::Start(0)).await?;
        flush_file_caches().await?;

        self.verify_checksum(&mut device, img_checksum).await?;

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

    pub async fn os_update(self) -> anyhow::Result<()> {
        todo!()
    }

    async fn write_to_device<W: AsyncWrite + Unpin>(
        &mut self,
        mut device: W,
    ) -> anyhow::Result<u64> {
        let mut buffer = vec![0u8; BUF_SIZE as usize];
        let mut total_read = 0;

        let img_crc = Crc::<u64>::new(&CRC_64_REDIS);
        let mut img_digest = img_crc.digest();

        let (size_sender, size_receiver) = channel::<u64>(32);
        tokio::spawn(run_progress_printer(
            self.size,
            self.progress_sender.clone(),
            size_receiver,
        ));

        let mut progress_update_guard = 0u64;

        // Read_exact and write_all is used here to enforce a certain write size to the `image_writer`.
        // This function could be further optimized to reduce the amount of awaiting reads/writes, e.g.
        // look at the implementation of `tokio::io::copy`. But in the case of an 'rockusb'
        // image_writer, writes that are misaligned with the sector-size of its device induces an extra
        // buffering penalty.
        while total_read < self.size {
            let buf_len = BUF_SIZE.min(self.size - total_read) as usize;
            let read_task = self.byte_stream.read_exact(&mut buffer[..buf_len]);
            let monitor_cancel = self.cancel.cancelled();

            tokio::select! {
                // self.bytes_stream is not guaranteed to be "cancel-safe". Canceling read_task
                // might result in a data loss. We allow this as a cancel aborts the whole file
                // file transfer
                res = read_task => ensure!(res? == buf_len),
                _ = monitor_cancel => bail!("write cancelled"),
            }

            total_read += buf_len as u64;

            // we accept sporadic lost progress updates or in worst case an error
            // inside the channel. It should never prevent the writing process from
            // completing.
            // Updates to the progress printer are throttled with an arbitrary
            // value.
            if progress_update_guard % 1000 == 0 {
                let _ = size_sender.try_send(total_read);
            }
            progress_update_guard += 1;

            img_digest.update(&buffer[..buf_len]);
            device
                .write_all(&buffer[..buf_len])
                .map_err(|e| anyhow::anyhow!("device write error: {}", e))
                .await?;
        }

        device.flush().await?;

        Ok(img_digest.finalize())
    }

    async fn verify_checksum<L>(&self, reader: L, img_checksum: u64) -> anyhow::Result<()>
    where
        L: AsyncRead + std::marker::Unpin,
    {
        let mut reader = io::BufReader::with_capacity(BUF_SIZE as usize, reader);

        let mut buffer = vec![0u8; BUF_SIZE as usize];
        let mut total_read = 0;

        let crc = Crc::<u64>::new(&CRC_64_REDIS);
        let mut digest = crc.digest();

        while total_read < self.size {
            let bytes_left = self.size - total_read;
            let buffer_size = buffer.len().min(bytes_left as usize);
            let read_task = reader.read(&mut buffer[..buffer_size]);
            let monitor_cancel = self.cancel.cancelled();

            let num_read = tokio::select! {
                res = read_task => res?,
                _ = monitor_cancel => bail!("checksum calculation is cancelled"),
            };

            if num_read == 0 {
                log::error!("read 0 bytes with {} bytes to go", bytes_left);
                bail!(FlashingError::IoError);
            }

            total_read += num_read as u64;
            digest.update(&buffer[..num_read]);
        }

        let dev_checksum = digest.finalize();
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
        Ok(())
    }
}

async fn run_progress_printer(
    img_len: u64,
    logging: Sender<FlashProgress>,
    mut read_reciever: Receiver<u64>,
) -> anyhow::Result<()> {
    let start_time = Instant::now();
    let mut last_print = 0;

    while let Some(total_read) = read_reciever.recv().await {
        let read_percent = 100 * total_read / img_len;

        let progress_counter = read_percent - last_print;
        if progress_counter >= PROGRESS_REPORT_PERCENT {
            #[allow(clippy::cast_precision_loss)] // This affects files > 4 exabytes long
            let read_proportion = (total_read as f64) / (img_len as f64);

            let duration = start_time.elapsed();
            let estimated_end = duration.div_f64(read_proportion);
            let estimated_left = estimated_end - duration;

            let est_seconds = estimated_left.as_secs() % 60;
            let est_minutes = (estimated_left.as_secs() / 60) % 60;

            let message = format!(
                "Progress: {:>2}%, estimated time left: {:02}:{:02}",
                read_percent, est_minutes, est_seconds,
            );

            logging
                .send(FlashProgress {
                    status: FlashStatus::Progress {
                        read_percent: read_percent as usize,
                        est_minutes,
                        est_seconds,
                    },
                    message,
                })
                .await
                .context("progress update error")?;
            last_print = read_percent;
        }
    }
    Ok(())
}

async fn flush_file_caches() -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open("/proc/sys/vm/drop_caches")
        .await?;

    // Free reclaimable slab objects and page cache
    file.write_u8(b'3').await
}
