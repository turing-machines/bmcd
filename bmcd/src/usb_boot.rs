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
mod rockusb;
mod rpiboot;
use self::{rockusb::RockusbBoot, rpiboot::RpiBoot};
use async_trait::async_trait;
use rusb::GlobalContext;
use std::{fmt::Display, path::PathBuf};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite};
use tracing::{info, warn};

pub trait DataTransport: AsyncRead + AsyncWrite + AsyncSeek + Send + Unpin {}
impl DataTransport for tokio::fs::File {}

#[async_trait]
pub trait UsbBoot: 'static + Send + Sync + Display {
    fn is_supported(&self, vid_pid: &(u16, u16)) -> bool;
    async fn load_as_block_device(
        &self,
        _device: &rusb::Device<GlobalContext>,
    ) -> Result<PathBuf, UsbBootError> {
        Err(UsbBootError::NotSupported)
    }

    async fn load_as_stream(
        &self,
        device: &rusb::Device<GlobalContext>,
    ) -> Result<Box<dyn DataTransport>, UsbBootError> {
        let path = self.load_as_block_device(device).await?;
        Ok(Box::new(
            tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .await?,
        ) as Box<dyn DataTransport>)
    }
}

pub struct NodeDrivers {
    backends: Vec<Box<dyn UsbBoot>>,
}

impl NodeDrivers {
    pub fn new() -> Self {
        NodeDrivers {
            backends: vec![Box::new(RpiBoot {}), Box::new(RockusbBoot {})],
        }
    }

    /// Due to the hardware implementation, only one node can be visible at any given time.
    /// This function tries to find the first USB device which exist a backend for.
    fn find_first(&self) -> Result<(rusb::Device<GlobalContext>, &dyn UsbBoot), UsbBootError> {
        tracing::info!("Checking for presence of a USB device...");
        let devices = rusb::devices()?;
        let mut backends = self.backends.iter().filter_map(|backend| {
            let found = devices.iter().find(|dev| {
                let Ok(descriptor) = dev.device_descriptor() else {
                    warn!("dropping {:?}, could not load descriptor", dev);
                    return false;
                };

                let vid_pid = (descriptor.vendor_id(), descriptor.product_id());
                info!(
                    "trying {:#06x}:{:#06x}",
                    descriptor.vendor_id(),
                    descriptor.product_id(),
                );
                let supported = backend.is_supported(&vid_pid);

                if supported {
                    info!(
                        "ID {:#06x}:{:#06x} {}",
                        descriptor.vendor_id(),
                        descriptor.product_id(),
                        backend
                    );
                }
                supported
            });
            found.map(|dev| (dev, backend.as_ref()))
        });

        backends.next().ok_or(UsbBootError::NotSupported)
    }

    pub async fn load_as_block_device(&self) -> Result<PathBuf, UsbBootError> {
        let (device, driver) = self.find_first()?;
        driver.load_as_block_device(&device).await
    }

    pub async fn load_as_stream(&self) -> Result<Box<dyn DataTransport>, UsbBootError> {
        let (device, driver) = self.find_first()?;
        driver.load_as_stream(&device).await
    }
}

#[derive(Error, Debug)]
pub enum UsbBootError {
    #[error("Compute module's USB interface not found or supported")]
    NotSupported,
    #[error("USB")]
    RusbError(#[from] rusb::Error),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error("Error loading USB device: {0}")]
    InternalError(String),
}

impl UsbBootError {
    pub fn internal_error<E: ToString>(error: E) -> UsbBootError {
        UsbBootError::InternalError(error.to_string())
    }
}
