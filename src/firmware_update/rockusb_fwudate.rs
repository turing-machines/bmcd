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
use super::{
    transport::{FwUpdateTransport, StdFwUpdateTransport, StdTransportWrapper},
    FwUpdateError, NodeBackend,
};
use crate::hal::usb;
use async_trait::async_trait;
use log::info;
use rockfile::boot::{
    RkBootEntry, RkBootEntryBytes, RkBootHeader, RkBootHeaderBytes, RkBootHeaderEntry,
};
use rockusb::libusb::{Transport, TransportIO};
use rusb::{DeviceDescriptor, GlobalContext};
use std::{mem::size_of, ops::Range, path::PathBuf, time::Duration};

const SPL_LOADER_RK3588: &[u8] = include_bytes!("./rk3588_spl_loader_v1.08.111.bin");
pub const RK3588_VID_PID: (u16, u16) = (0x2207, 0x350b);

pub struct RockusbBackend;

#[async_trait]
impl NodeBackend for RockusbBackend {
    fn is_supported(&self, vid_pid: &(u16, u16)) -> bool {
        vid_pid == &RK3588_VID_PID
    }

    async fn load_as_block_device(
        &self,
        device: &rusb::Device<GlobalContext>,
    ) -> Result<Option<std::path::PathBuf>, FwUpdateError> {
        if BootMode::Maskrom == device.device_descriptor()?.into() {
            info!("Maskrom mode detected. loading usb-plug..");
            let mut transport = Transport::from_usb_device(device.open()?)
                .map_err(FwUpdateError::internal_error)?;
            let file = download_boot(&mut transport).await?;
            return Ok(Some(file));
        }

        Ok(None)
    }

    async fn load_as_stream(
        &self,
        device: &rusb::Device<GlobalContext>,
    ) -> Result<Box<dyn FwUpdateTransport>, FwUpdateError> {
        let transport =
            Transport::from_usb_device(device.open()?).map_err(FwUpdateError::internal_error)?;
        Ok(Box::new(StdTransportWrapper::new(
            transport.into_io().map_err(FwUpdateError::internal_error)?,
        )))
    }
}

impl StdFwUpdateTransport for TransportIO<Transport> {}

async fn download_boot(transport: &mut Transport) -> Result<PathBuf, FwUpdateError> {
    let boot_entries = parse_boot_entries(SPL_LOADER_RK3588)?;
    load_boot_entries(transport, boot_entries).await?;
    // Rockchip will reconnect to USB, back off a bit
    tokio::time::sleep(Duration::from_secs(3)).await;
    usb::get_device_path(&["Rockchip"])
        .await
        .map_err(FwUpdateError::internal_error)
}

fn parse_boot_entries(
    raw_boot_bytes: &'static [u8],
) -> Result<impl Iterator<Item = (u16, u32, &[u8])>, FwUpdateError> {
    let boot_header_raw = raw_boot_bytes[0..size_of::<RkBootHeaderBytes>()]
        .try_into()
        .expect("enough bytes to read boot header");
    let boot_header = RkBootHeader::from_bytes(boot_header_raw).ok_or(
        FwUpdateError::InternalError("Boot header loader corrupt".to_string()),
    )?;

    let entry_471 = parse_boot_header_entry(0x471, raw_boot_bytes, boot_header.entry_471);
    let entry_472 = parse_boot_header_entry(0x472, raw_boot_bytes, boot_header.entry_472);
    Ok(entry_471.chain(entry_472))
}

fn parse_boot_header_entry(
    entry_type: u16,
    blob: &[u8],
    header: RkBootHeaderEntry,
) -> impl Iterator<Item = (u16, u32, &[u8])> {
    let mut results = Vec::new();
    let mut range = header.offset..header.offset + header.size as u32;
    for _ in 0..header.count as usize {
        let boot_entry = parse_boot_entry(blob, &range);
        let name = String::from_utf16(boot_entry.name.as_slice()).unwrap_or_default();
        log::debug!(
            "Found boot entry [{:x}] {} {}",
            entry_type,
            name,
            humansize::format_size(boot_entry.data_size, humansize::DECIMAL)
        );

        if boot_entry.size == 0 {
            log::debug!("skipping, size == 0 of {}", name);
            continue;
        }

        let start = boot_entry.data_offset as usize;
        let end = start + boot_entry.data_size as usize;
        let data = &blob[start..end];
        results.push((entry_type, boot_entry.data_delay, data));

        range.start += header.size as u32;
        range.end += header.size as u32;
    }

    results.into_iter()
}

fn parse_boot_entry(blob: &[u8], range: &Range<u32>) -> RkBootEntry {
    let boot_entry_size = size_of::<RkBootEntryBytes>();
    let narrowed_range = range.start as usize..range.start as usize + boot_entry_size;
    let narrowed_slice: RkBootEntryBytes = blob[narrowed_range]
        .try_into()
        .expect("valid range inside blob");
    RkBootEntry::from_bytes(&narrowed_slice)
}

async fn load_boot_entries(
    transport: &mut Transport,
    iterator: impl Iterator<Item = (u16, u32, &'static [u8])>,
) -> Result<(), FwUpdateError> {
    let mut size = 0;
    for (area, delay, data) in iterator {
        transport
            .write_maskrom_area(area, data)
            .map_err(FwUpdateError::internal_error)?;
        tokio::time::sleep(Duration::from_millis(delay.into())).await;
        size += data.len();
    }
    log::debug!("written {} bytes", size);
    Ok(())
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum BootMode {
    Maskrom = 0,
    Loader = 1,
}

impl From<DeviceDescriptor> for BootMode {
    fn from(dd: DeviceDescriptor) -> BootMode {
        match dd.usb_version().sub_minor() & 0x1 {
            0 => BootMode::Maskrom,
            1 => BootMode::Loader,
            _ => unreachable!(),
        }
    }
}

/// Detecting maskrom is tricky, because the USB descriptor used by the maskrom
/// is extremely similar to the one used by Rockchip's "usbplug" stub. The only
/// difference is that the maskrom descriptor has no device strings, while the
/// "usbplug" stub (and U-Boot bootloaders in Rockusb mode) populate at least
/// one of them.
#[allow(unused)]
fn requires_usb_plug(device: &rusb::Device<GlobalContext>) -> rusb::Result<bool> {
    let desc = device.device_descriptor()?;
    let handle = device.open()?;

    let timeout = Duration::from_secs(1);
    let language = handle.read_languages(timeout)?[0];

    let result = handle
        .read_manufacturer_string(language, &desc, timeout)?
        .is_empty()
        && handle
            .read_product_string(language, &desc, timeout)?
            .is_empty()
        && handle
            .read_serial_number_string(language, &desc, timeout)?
            .is_empty();
    Ok(result)
}
