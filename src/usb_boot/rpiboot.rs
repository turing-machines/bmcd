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
use super::UsbBoot;
use crate::{usb_boot::UsbBootError, utils::get_device_path};
use async_trait::async_trait;
use std::{fmt::Display, time::Duration};
use tokio::time::sleep;

const VID_PID: (u16, u16) = (0x0a5c, 0x2711);

pub struct RpiBoot;

#[async_trait]
impl UsbBoot for RpiBoot {
    fn is_supported(&self, vid_pid: &(u16, u16)) -> bool {
        vid_pid == &VID_PID
    }

    async fn load_as_block_device(
        &self,
        _device: &rusb::Device<rusb::GlobalContext>,
    ) -> Result<std::path::PathBuf, UsbBootError> {
        load_rpi_boot().await?;
        log::info!("Checking for presence of a device file ('RPi-MSD-.*')...");
        get_device_path(&["RPi-MSD-"])
            .await
            .map_err(UsbBootError::internal_error)
    }
}

impl Display for RpiBoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rustpiboot")
    }
}

async fn load_rpi_boot() -> Result<(), UsbBootError> {
    let options = rustpiboot::Options {
        delay: 500 * 1000,
        ..Default::default()
    };

    rustpiboot::boot(options).map_err(|err| {
        UsbBootError::internal_error(format!(
            "Failed to reboot {:?} as USB MSD: {:?}",
            VID_PID, err
        ))
    })?;

    sleep(Duration::from_secs(3)).await;
    Ok(())
}
