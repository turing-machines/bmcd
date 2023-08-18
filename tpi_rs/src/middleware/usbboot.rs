use std::fmt::{self, Display};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use crc::{Crc, CRC_64_REDIS};
use rusb::{Device, GlobalContext, UsbContext};
use tokio::fs;
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::Sender;

const BUF_SIZE: usize = 8 * 1024;
const PROGRESS_REPORT_PERCENT: u64 = 5;

#[derive(Debug, Clone, Copy)]
pub enum FlashingError {
    InvalidArgs,
    DeviceNotFound,
    GpioError,
    UsbError,
    IoError,
    ChecksumMismatch,
}

impl fmt::Display for FlashingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlashingError::InvalidArgs => {
                write!(f, "A specified node does not exist or image is not valid")
            }
            FlashingError::DeviceNotFound => write!(f, "Device not found"),
            FlashingError::GpioError => write!(f, "Error toggling GPIO lines"),
            FlashingError::UsbError => write!(f, "Error enumerating USB devices"),
            FlashingError::IoError => write!(f, "File IO error"),
            FlashingError::ChecksumMismatch => {
                write!(f, "Failed to verify image after writing to the node")
            }
        }
    }
}

impl std::error::Error for FlashingError {}

pub trait FlashingErrorExt<T, E: Display> {
    fn map_err_into_logged_usb(self, logging: &Sender<FlashProgress>) -> Result<T, FlashingError>;
    fn map_err_into_logged_io(self, logging: &Sender<FlashProgress>) -> Result<T, FlashingError>;
}

impl<T, E: Display> FlashingErrorExt<T, E> for Result<T, E> {
    fn map_err_into_logged_usb(self, logging: &Sender<FlashProgress>) -> Result<T, FlashingError> {
        self.map_err(|e| {
            logging
                .try_send(FlashProgress {
                    status: FlashStatus::Error(FlashingError::UsbError),
                    message: format!("{}", e),
                })
                .expect("logging channel to be open");
            FlashingError::UsbError
        })
    }
    fn map_err_into_logged_io(self, logging: &Sender<FlashProgress>) -> Result<T, FlashingError> {
        self.map_err(|e| {
            logging
                .try_send(FlashProgress {
                    status: FlashStatus::Error(FlashingError::IoError),
                    message: format!("{}", e),
                })
                .expect("logging channel to be open");
            FlashingError::IoError
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FlashStatus {
    Idle,
    Setup,
    Progress {
        read_percent: u64,
        est_minutes: u64,
        est_seconds: u64,
    },
    Error(FlashingError),
    Done,
}

#[derive(Debug, Clone)]
pub struct FlashProgress {
    pub status: FlashStatus,
    pub message: String,
}

impl Display for FlashProgress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

pub(crate) fn get_usb_devices<'a, I: IntoIterator<Item = &'a (u16, u16)>>(
    filter: I,
) -> std::result::Result<Vec<Device<GlobalContext>>, FlashingError> {
    let all_devices = rusb::DeviceList::new().map_err(|err| {
        log::error!("failed to get USB device list: {}", err);
        FlashingError::UsbError
    })?;

    let filter = filter.into_iter().collect::<Vec<&'a (u16, u16)>>();
    let devices = all_devices
        .iter()
        .filter_map(|dev| {
            let desc = dev.device_descriptor().ok()?;
            let this = (desc.vendor_id(), desc.product_id());
            filter.contains(&&this).then_some(dev)
        })
        .collect::<Vec<Device<GlobalContext>>>();

    Ok(devices)
}

#[allow(dead_code)]
fn map_to_serial<T: UsbContext>(dev: &rusb::Device<T>) -> anyhow::Result<String> {
    let desc = dev.device_descriptor()?;
    let handle = dev.open()?;
    let timeout = Duration::from_secs(1);
    let language = handle.read_languages(timeout)?;
    handle
        .read_serial_number_string(language.first().copied().unwrap(), &desc, timeout)
        .context("error reading serial")
}

pub(crate) fn extract_one_device<T>(devices: &[T]) -> Result<&T, FlashingError> {
    match devices.len() {
        1 => Ok(devices.first().unwrap()),
        0 => {
            log::error!("No supported devices found");
            Err(FlashingError::DeviceNotFound)
        }
        n => {
            log::error!("Several supported devices found: found {}, expected 1", n);
            Err(FlashingError::GpioError)
        }
    }
}

pub(crate) async fn write_to_device<P, W>(
    image_path: P,
    async_writer: &mut W,
    sender: &Sender<FlashProgress>,
) -> Result<(u64, u64)>
where
    W: AsyncWrite + std::marker::Unpin,
    P: AsRef<Path>,
{
    let img_file = fs::File::open(image_path).await?;
    let img_len = img_file.metadata().await?.len();
    let mut reader = io::BufReader::with_capacity(BUF_SIZE, img_file);
    let mut writer = io::BufWriter::with_capacity(BUF_SIZE, async_writer);

    let mut buffer = vec![0u8; BUF_SIZE];
    let mut total_read = 0;

    let progress_interval = img_len / 100 * PROGRESS_REPORT_PERCENT;
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

            print_progress(total_read, img_len, start_time, sender).await?;
        }

        img_digest.update(&buffer[..num_read]);

        let mut pos = 0;

        while pos < num_read {
            let num_written = writer.write(&buffer[pos..num_read]).await?;
            pos += num_written;
        }
    }

    if total_read < img_len {
        log::error!(
            "Partial read of image file: total {} B, read {} B",
            img_len,
            total_read
        );
        bail!(FlashingError::IoError);
    }

    writer.flush().await?;

    Ok((img_len, img_digest.finalize()))
}

async fn print_progress(
    total_read: u64,
    img_len: u64,
    start_time: Instant,
    sender: &Sender<FlashProgress>,
) -> Result<()> {
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

pub(crate) async fn verify_checksum<R>(
    img_checksum: u64,
    img_len: u64,
    reader: &mut R,
    sender: &Sender<FlashProgress>,
) -> Result<()>
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
async fn calc_file_checksum<R>(reader: &mut R, to_read: u64) -> anyhow::Result<u64>
where
    R: AsyncRead + std::marker::Unpin,
{
    let mut reader = io::BufReader::with_capacity(BUF_SIZE, reader);

    let mut buffer = vec![0u8; BUF_SIZE];
    let mut total_read = 0;

    let crc = Crc::<u64>::new(&CRC_64_REDIS);
    let mut digest = crc.digest();

    loop {
        let num_read = reader.read(&mut buffer).await?;
        total_read += num_read as u64;

        if num_read == 0 || total_read > to_read {
            break;
        }

        digest.update(&buffer[..num_read]);
    }

    if total_read < to_read {
        log::error!(
            "Partial read of file: total {} B, read {} B",
            to_read,
            total_read
        );
        bail!(FlashingError::IoError);
    }

    Ok(digest.finalize())
}
