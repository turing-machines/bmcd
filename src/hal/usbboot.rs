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
use crate::firmware_update::FlashingError;
use anyhow::{Context, Result};
use rusb::{Device, GlobalContext, UsbContext};
use std::time::Duration;

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
