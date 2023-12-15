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
use crate::firmware_update::FwUpdateError;
use anyhow::{Context, Result};
use rusb::{Device, GlobalContext, UsbContext};
use std::{path::PathBuf, time::Duration};

pub(crate) fn get_usb_devices() -> std::result::Result<Vec<Device<GlobalContext>>, FwUpdateError> {
    let all_devices = rusb::DeviceList::new()?
        .iter()
        .take(1)
        .collect::<Vec<Device<GlobalContext>>>();

    log::debug!("matches:{:?}", all_devices);
    Ok(all_devices)
}

pub(crate) fn extract_one_device<T>(devices: &[T]) -> Result<&T, FwUpdateError> {
    match devices.len() {
        1 => Ok(devices.first().unwrap()),
        0 => Err(FwUpdateError::NoDevices),
        n => Err(FwUpdateError::MultipleDevicesFound(n)),
    }
}

pub async fn get_device_path(_allowed_vendors: &[&str]) -> Result<PathBuf, FwUpdateError> {
    Ok(PathBuf::from("/tmp/stubbed"))
}
