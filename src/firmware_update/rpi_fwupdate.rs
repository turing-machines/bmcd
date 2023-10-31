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
use crate::{firmware_update::FwUpdateError, hal::usb::get_device_path};
use std::time::Duration;
use tokio::{fs::File, time::sleep};

pub const VID_PID: (u16, u16) = (0x0a5c, 0x2711);
pub async fn new_rpi_transport() -> Result<File, FwUpdateError> {
    let options = rustpiboot::Options {
        delay: 500 * 1000,
        ..Default::default()
    };

    log::info!("Rebooting as a USB mass storage device...");

    rustpiboot::boot(options).map_err(|err| {
        FwUpdateError::internal_error(format!(
            "Failed to reboot {:?} as USB MSD: {:?}",
            VID_PID, err
        ))
    })?;

    sleep(Duration::from_secs(3)).await;

    log::info!("Checking for presence of a device file...");
    let device_path = get_device_path(&["RPi-MSD-"]).await?;
    let msd_device = tokio::fs::OpenOptions::new()
        .write(true)
        .read(true)
        .open(&device_path)
        .await?;

    Ok(msd_device)
}
