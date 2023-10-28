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
use self::rockusb_fwudate::new_rockusb_transport;
mod rpi_fwupdate;
use rpi_fwupdate::new_rpi_transport;
use thiserror::Error;
pub mod transport;
use self::transport::FwUpdateTransport;
use futures::future::BoxFuture;
use once_cell::sync::Lazy;
use rusb::GlobalContext;
use std::collections::HashMap;

pub static SUPPORTED_DEVICES: Lazy<HashMap<(u16, u16), FactoryItemCreator>> = Lazy::new(|| {
    let mut creators = HashMap::<(u16, u16), FactoryItemCreator>::new();
    creators.insert(
        rpi_fwupdate::VID_PID,
        Box::new(|_| Box::pin(async move { new_rpi_transport().await.map(Into::into) })),
    );

    creators.insert(
        rockusb_fwudate::RK3588_VID_PID,
        Box::new(|device| {
            let clone = device.clone();
            Box::pin(async move { new_rockusb_transport(clone).await.map(Into::into) })
        }),
    );

    creators
});

pub type FactoryItem = BoxFuture<'static, Result<Box<dyn FwUpdateTransport>, FwUpdateError>>;
pub type FactoryItemCreator =
    Box<dyn Fn(&rusb::Device<GlobalContext>) -> FactoryItem + Send + Sync>;

pub fn fw_update_transport(
    device: &rusb::Device<GlobalContext>,
) -> Result<FactoryItem, FwUpdateError> {
    let descriptor = device.device_descriptor()?;
    let vid_pid = (descriptor.vendor_id(), descriptor.product_id());

    SUPPORTED_DEVICES
        .get(&vid_pid)
        .map(|creator| creator(device))
        .ok_or(FwUpdateError::NoDriver(device.clone()))
}

#[derive(Error, Debug)]
pub enum FwUpdateError {
    #[error("No supported devices found")]
    NoDevices,
    #[error("No MSD USB devices found")]
    NoMsdDevices,
    #[error("Several supported devices found: found {0:?}, expected 1")]
    MultipleDevicesFound(usize),
    #[error("USB")]
    RusbError(#[from] rusb::Error),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error("Error loading as USB MSD: {0}")]
    InternalError(String),
    #[error("integrity check of written image failed")]
    ChecksumMismatch,
    #[error("no firmware update driver available for {0:?}")]
    NoDriver(rusb::Device<GlobalContext>),
}

impl FwUpdateError {
    pub fn internal_error<E: ToString>(error: E) -> FwUpdateError {
        FwUpdateError::InternalError(error.to_string())
    }
}
