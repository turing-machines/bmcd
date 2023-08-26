use super::bmc_application::UsbConfig;
use crate::app::bmc_application::BmcApplication;
use crate::middleware::firmware_update::FlashProgress;
use crate::middleware::firmware_update::FlashStatus;
use crate::middleware::firmware_update::FlashingError;
use crate::middleware::firmware_update::SUPPORTED_DEVICES;
use crate::middleware::NodeId;
use crate::middleware::UsbRoute;
use anyhow::bail;
use anyhow::Context;
use crc::{Crc, CRC_64_REDIS};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::io::AsyncSeekExt;
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tokio::time::Instant;

const REBOOT_DELAY: Duration = Duration::from_millis(500);
const BUF_SIZE: usize = 8 * 1024;
const PROGRESS_REPORT_PERCENT: u64 = 5;

pub struct FlashContext<R: AsyncRead> {
    pub filename: String,
    pub size: usize,
    pub node: NodeId,
    pub byte_stream: R,
    pub bmc: Arc<BmcApplication>,
    pub progress_sender: Sender<FlashProgress>,
}

pub async fn flash_node<R: AsyncRead + Unpin>(context: FlashContext<R>) -> anyhow::Result<()> {
    let bmc = context.bmc;
    let node = context.node;
    let progress_sender = context.progress_sender;
    let filename = context.filename;
    let image = context.byte_stream;
    let image_size = context.size as u64;

    let mut driver = bmc
        .configure_node_for_fwupgrade(
            node,
            UsbRoute::Bmc,
            progress_sender.clone(),
            SUPPORTED_DEVICES.keys(),
        )
        .await?;

    let mut progress_state = FlashProgress {
        message: String::new(),
        status: FlashStatus::Setup,
    };

    progress_state.message = format!("Writing {:?}", filename);
    progress_sender.send(progress_state.clone()).await?;

    let (img_len, img_checksum) =
        write_to_device(image, image_size, &mut driver, &progress_sender).await?;

    progress_state.message = String::from("Verifying checksum...");
    progress_sender.send(progress_state.clone()).await?;

    driver.seek(std::io::SeekFrom::Start(0)).await?;

    verify_checksum(img_checksum, img_len, &mut driver, &progress_sender).await?;

    progress_state.message = String::from("Flashing successful, restarting device...");
    progress_sender.send(progress_state.clone()).await?;

    bmc.activate_slot(!node.to_bitfield(), node.to_bitfield())
        .await?;

    //TODO: we probably want to restore the state prior flashing
    bmc.configure_usb(UsbConfig::UsbA(node, false)).await?;

    sleep(REBOOT_DELAY).await;

    bmc.activate_slot(node.to_bitfield(), node.to_bitfield())
        .await?;

    progress_state.message = String::from("Done");
    progress_sender.send(progress_state).await?;
    Ok(())
}

async fn write_to_device<R, W>(
    image: R,
    image_len: u64,
    async_writer: &mut W,
    sender: &Sender<FlashProgress>,
) -> anyhow::Result<(u64, u64)>
where
    W: AsyncWrite + std::marker::Unpin,
    R: AsyncRead + std::marker::Unpin,
{
    let mut reader = image;
    let writer = async_writer;

    let mut buffer = vec![0u8; BUF_SIZE];
    let mut total_read = 0;

    let progress_interval = image_len / 100 * PROGRESS_REPORT_PERCENT;
    let mut progress_counter = 0;

    let img_crc = Crc::<u64>::new(&CRC_64_REDIS);
    let mut img_digest = img_crc.digest();

    let start_time = Instant::now();

    while let Ok(num_read) = reader.read(&mut buffer).await {
        if num_read == 0 {
            break;
        }

        total_read += num_read as u64;

        progress_counter += num_read as u64;
        if progress_counter > progress_interval {
            progress_counter -= progress_interval;

            print_progress(total_read, image_len, start_time, sender).await?;
        }

        img_digest.update(&buffer[..num_read]);

        let mut pos = 0;

        while pos < num_read {
            let num_written = writer.write(&buffer[pos..num_read]).await?;
            pos += num_written;
        }
    }

    if total_read < image_len {
        log::error!(
            "Partial read of image file: total {} B, read {} B",
            image_len,
            total_read
        );
        bail!(FlashingError::IoError);
    }

    writer.flush().await?;

    Ok((image_len, img_digest.finalize()))
}

async fn print_progress(
    total_read: u64,
    img_len: u64,
    start_time: Instant,
    sender: &Sender<FlashProgress>,
) -> anyhow::Result<()> {
    let read_percent = 100 * total_read / img_len;

    let duration = start_time.elapsed();

    #[allow(clippy::cast_precision_loss)] // This affects files > 4 exabytes long
    let read_proportion = (total_read as f64) / (img_len as f64);

    let estimated_end = duration.div_f64(read_proportion);
    let estimated_left = estimated_end - duration;

    let est_seconds = estimated_left.as_secs() % 60;
    let est_minutes = (estimated_left.as_secs() / 60) % 60;

    let message = format!(
        "Progress: {:>2}%, estimated time left: {:02}:{:02}",
        read_percent, est_minutes, est_seconds,
    );

    sender
        .send(FlashProgress {
            status: FlashStatus::Progress {
                read_percent,
                est_minutes,
                est_seconds,
            },
            message,
        })
        .await
        .context("progress update error")
}

async fn verify_checksum<R>(
    img_checksum: u64,
    img_len: u64,
    reader: &mut R,
    sender: &Sender<FlashProgress>,
) -> anyhow::Result<()>
where
    R: AsyncRead + std::marker::Unpin,
{
    flush_file_caches().await?;

    let dev_checksum = calc_file_checksum(reader, img_len).await?;

    if img_checksum == dev_checksum {
        Ok(())
    } else {
        sender
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
}

async fn flush_file_caches() -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open("/proc/sys/vm/drop_caches")
        .await?;

    // Free reclaimable slab objects and page cache
    file.write_u8(b'3').await
}

// This function and `write_to_device()` could be merged into one with an optional callback for
// every chunk read, but async closures are unstable and async blocks seem to require a Mutex.
async fn calc_file_checksum<R>(reader: &mut R, total_size: u64) -> anyhow::Result<u64>
where
    R: AsyncRead + std::marker::Unpin,
{
    let mut reader = io::BufReader::with_capacity(BUF_SIZE, reader);

    let mut buffer = vec![0u8; BUF_SIZE];
    let mut total_read = 0;

    let crc = Crc::<u64>::new(&CRC_64_REDIS);
    let mut digest = crc.digest();

    while total_read < total_size {
        let bytes_left = total_size - total_read;
        let buffer_size = buffer.len().min(bytes_left as usize);
        let num_read = reader.read(&mut buffer[..buffer_size]).await?;
        if num_read == 0 {
            log::error!("read 0 bytes with {} bytes to go", bytes_left);
            bail!(FlashingError::IoError);
        }

        total_read += num_read as u64;
        digest.update(&buffer[..num_read]);
    }

    Ok(digest.finalize())
}
