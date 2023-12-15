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
mod rockusb_fwudate;
use self::{rockusb_fwudate::RockusbBackend, rpi_fwupdate::RpiBackend};
mod rpi_fwupdate;
use thiserror::Error;
pub mod transport;
use self::transport::FwUpdateTransport;
use async_trait::async_trait;
use rusb::GlobalContext;
use std::path::PathBuf;

pub struct NodeDrivers {
    backends: Vec<Box<dyn NodeBackend>>,
}

impl NodeDrivers {
    pub fn new() -> Self {
        NodeDrivers {
            backends: vec![Box::new(RpiBackend {}), Box::new(RockusbBackend {})],
        }
    }

    /// Due to the hardware implementation, only one node can be visible at any given time.
    /// This function tries to find the first USB device which exist a backend for.
    fn find_first(&self) -> Result<(rusb::Device<GlobalContext>, &dyn NodeBackend), FwUpdateError> {
        log::info!("Checking for presence of a USB device...");
        let devices = rusb::devices().map_err(FwUpdateError::internal_error)?;
        let mut backends = self.backends.iter().filter_map(|backend| {
            let found = devices.iter().find(|dev| {
                let Ok(descriptor) = dev.device_descriptor() else {
                    return false;
                };

                let vid_pid = (descriptor.vendor_id(), descriptor.product_id());
                backend.is_supported(&vid_pid)
            });
            found.map(|dev| (dev, backend.as_ref()))
        });

        backends.next().ok_or(FwUpdateError::NotSupported)
    }

    pub async fn load_as_block_device(&self) -> Result<PathBuf, FwUpdateError> {
        let (device, driver) = self.find_first()?;
        driver
            .load_as_block_device(&device)
            .await?
            .ok_or(FwUpdateError::NotSupported)
    }

    pub async fn load_as_stream(&self) -> Result<Box<dyn FwUpdateTransport>, FwUpdateError> {
        let (device, driver) = self.find_first()?;
        match driver
            .load_as_block_device(&device)
            .await?
            .ok_or(FwUpdateError::NotSupported)
        {
            Ok(file) => Ok(Box::new(
                tokio::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&file)
                    .await?,
            )),
            Err(_) => driver.load_as_stream(&device).await,
        }
    }
}

#[derive(Error, Debug)]
pub enum FwUpdateError {
    #[error("Compute module not supported")]
    NotSupported,
    #[error("USB")]
    RusbError(#[from] rusb::Error),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error("Error loading USB device: {0}")]
    InternalError(String),
}

impl FwUpdateError {
    pub fn internal_error<E: ToString>(error: E) -> FwUpdateError {
        FwUpdateError::InternalError(error.to_string())
    }
}

#[async_trait]
pub trait NodeBackend: 'static + Send + Sync {
    fn is_supported(&self, vid_pid: &(u16, u16)) -> bool;
    async fn load_as_block_device(
        &self,
        device: &rusb::Device<GlobalContext>,
    ) -> Result<Option<PathBuf>, FwUpdateError>;

    async fn load_as_stream(
        &self,
        device: &rusb::Device<GlobalContext>,
    ) -> Result<Box<dyn FwUpdateTransport>, FwUpdateError>;
}
