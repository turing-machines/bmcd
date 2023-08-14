use super::transport::{StdFwUpdateTransport, StdTransportWrapper};
use crate::middleware::usbboot::{
    self, FlashProgress, FlashStatus, FlashingError, FlashingErrorExt,
};
use anyhow::Context;
use log::info;
use rockfile::boot::{
    RkBootEntry, RkBootEntryBytes, RkBootHeader, RkBootHeaderBytes, RkBootHeaderEntry,
};
use rockusb::libusb::BootMode;
use rockusb::libusb::{Transport, TransportIO};
use rusb::GlobalContext;
use std::{mem::size_of, ops::Range, time::Duration};
use tokio::sync::mpsc::Sender;

const SPL_LOADER_RK3588: &[u8] = include_bytes!("./rk3588_spl_loader_v1.08.111.bin");

pub const RK3588_VID_PID: (u16, u16) = (0x2207, 0x350b);
pub async fn new_rockusb_transport(
    device: rusb::Device<GlobalContext>,
    logging: &Sender<FlashProgress>,
) -> Result<StdTransportWrapper<TransportIO<Transport>>, FlashingError> {
    let mut transport = Transport::from_usb_device(device.open().map_err_into_logged_usb(logging)?)
        .map_err(|_| FlashingError::UsbError)?;

    if let Ok(BootMode::MaskedRom) = transport.boot_mode() {
        info!("MaskedRom mode detected. loading usb-plug..");
        transport = download_boot(&mut transport, logging).await?;
        logging
            .try_send(FlashProgress {
                status: FlashStatus::Setup,
                message: format!(
                    "Chip Info bytes: {:0x?}",
                    transport
                        .chip_info()
                        .map_err_into_logged_usb(logging)?
                        .inner()
                ),
            })
            .map_err(|_| FlashingError::IoError)?;
    }

    Ok(StdTransportWrapper::new(
        transport.into_io().map_err_into_logged_io(logging)?,
    ))
}

impl StdFwUpdateTransport for TransportIO<Transport> {}

async fn download_boot(
    transport: &mut Transport,
    logging: &Sender<FlashProgress>,
) -> Result<Transport, FlashingError> {
    let boot_entries = parse_boot_entries(SPL_LOADER_RK3588).map_err_into_logged_io(logging)?;
    load_boot_entries(transport, boot_entries)
        .await
        .map_err_into_logged_io(logging)?;
    // Rockchip will reconnect to USB, back off a bit
    tokio::time::sleep(Duration::from_secs(1)).await;

    let devices = usbboot::get_usb_devices([&RK3588_VID_PID]).map_err_into_logged_usb(logging)?;
    log::debug!("re-enumerated usb devices={:?}", devices);
    assert!(devices.len() == 1);

    Transport::from_usb_device(devices[0].open().map_err_into_logged_usb(logging)?)
        .map_err_into_logged_usb(logging)
}

fn parse_boot_entries(
    raw_boot: &'static [u8],
) -> anyhow::Result<impl Iterator<Item = (u16, u32, &[u8])>> {
    let boot_header_raw = raw_boot[0..size_of::<RkBootHeaderBytes>()].try_into()?;
    let boot_header =
        RkBootHeader::from_bytes(boot_header_raw).context("Boot header loader corrupt")?;

    let entry_471 = parse_boot_header_entry::<0x471>(raw_boot, boot_header.entry_471)?;
    let entry_472 = parse_boot_header_entry::<0x472>(raw_boot, boot_header.entry_472)?;
    Ok(entry_471.chain(entry_472))
}

fn parse_boot_header_entry<const ENTRY: u16>(
    blob: &[u8],
    header: RkBootHeaderEntry,
) -> anyhow::Result<impl Iterator<Item = (u16, u32, &[u8])>> {
    let mut results = Vec::new();
    let mut range = header.offset..header.offset + header.size as u32;
    for _ in 0..header.count as usize {
        let boot_entry = parse_boot_entry(blob, &range)?;
        let name = String::from_utf16(boot_entry.name.as_slice()).unwrap_or_default();
        log::debug!(
            "Found boot entry [{:x}] {} {} KiB",
            ENTRY,
            name,
            boot_entry.data_size / 1024,
        );

        if boot_entry.size == 0 {
            log::debug!("skipping, size == 0 of {}", name);
            continue;
        }

        let start = boot_entry.data_offset as usize;
        let end = start + boot_entry.data_size as usize;
        let data = &blob[start..end];
        results.push((ENTRY, boot_entry.data_delay, data));

        range.start += header.size as u32;
        range.end += header.size as u32;
    }

    Ok(results.into_iter())
}

fn parse_boot_entry(blob: &[u8], range: &Range<u32>) -> anyhow::Result<RkBootEntry> {
    let boot_entry_size = size_of::<RkBootEntryBytes>();
    let narrowed_range = range.start as usize..range.start as usize + boot_entry_size;
    let narrowed_slice: RkBootEntryBytes = blob[narrowed_range].try_into()?;
    Ok(RkBootEntry::from_bytes(&narrowed_slice))
}

async fn load_boot_entries(
    transport: &mut Transport,
    iterator: impl Iterator<Item = (u16, u32, &'static [u8])>,
) -> anyhow::Result<()> {
    let mut size = 0;
    for (area, delay, data) in iterator {
        transport.write_maskrom_area(area, data)?;
        tokio::time::sleep(Duration::from_millis(delay.into())).await;
        size += data.len();
    }
    log::debug!("written {} bytes", size);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::{
        firmware_update::SUPPORTED_DEVICES,
        usbboot::{self, get_usb_devices},
    };
    use log::LevelFilter;
    use simple_logger::SimpleLogger;
    use std::fmt::Display;
    use tokio::sync::mpsc::{channel, Receiver};

    #[ignore = "This test is not designed to be run in a test-pipeline.\
    Enable to debug USB with your own computer as USB host "]
    #[tokio::test]
    async fn test_rockusb() -> anyhow::Result<()> {
        const ARBRITRARY_IMG: &str =
            "/home/svenr/Downloads/Armbian_23.5.1_Rock-5b_jammy_legacy_5.10.160.img";

        SimpleLogger::new()
            .with_level(LevelFilter::Trace)
            .with_colors(true)
            .init()
            .expect("failed to initialize logger");

        let usb_device = get_usb_devices(SUPPORTED_DEVICES.keys())?[0].clone();
        let (logging, recv) = channel(64);
        let handle = logging_sink(recv);

        let mut driver = new_rockusb_transport(usb_device, &logging).await;
        if let Ok(ref mut writer) = driver {
            let (img_len, checksum) =
                usbboot::write_to_device(ARBRITRARY_IMG, writer, &logging).await?;
            usbboot::verify_checksum(checksum, img_len, writer, &logging).await?;
        }
        drop(logging);
        handle.await.context("logging error")
    }

    fn logging_sink<T: Display + Send + 'static>(
        mut receiver: Receiver<T>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(msg) = receiver.recv().await {
                log::info!("{}", msg);
            }
        })
    }
}
