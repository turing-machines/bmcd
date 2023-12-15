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
use super::{transport::FwUpdateTransport, NodeBackend};
use crate::{firmware_update::FwUpdateError, hal::usb::get_device_path};
use async_trait::async_trait;
use std::time::Duration;
use tokio::time::sleep;

const VID_PID: (u16, u16) = (0x0a5c, 0x2711);

pub struct RpiBackend;
#[async_trait]
impl NodeBackend for RpiBackend {
    fn is_supported(&self, vid_pid: &(u16, u16)) -> bool {
        vid_pid == &VID_PID
    }

    async fn load_as_block_device(
        &self,
        _device: &rusb::Device<rusb::GlobalContext>,
    ) -> Result<Option<std::path::PathBuf>, FwUpdateError> {
        load_rpi_boot().await?;
        log::info!("Checking for presence of a device file ('RPi-MSD-.*')...");
        get_device_path(&["RPi-MSD-"])
            .await
            .map(Some)
            .map_err(FwUpdateError::internal_error)
    }

    async fn load_as_stream(
        &self,
        device: &rusb::Device<rusb::GlobalContext>,
    ) -> Result<Box<dyn FwUpdateTransport>, FwUpdateError> {
        let device_path = self
            .load_as_block_device(device)
            .await?
            .expect("block device mode must be supported");
        let msd_device = tokio::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .open(&device_path)
            .await?;
        Ok(Box::new(msd_device))
    }
}

async fn load_rpi_boot() -> Result<(), FwUpdateError> {
    let options = rustpiboot::Options {
        delay: 500 * 1000,
        ..Default::default()
    };

    rustpiboot::boot(options).map_err(|err| {
        FwUpdateError::internal_error(format!(
            "Failed to reboot {:?} as USB MSD: {:?}",
            VID_PID, err
        ))
    })?;

    sleep(Duration::from_secs(3)).await;
    Ok(())
}
