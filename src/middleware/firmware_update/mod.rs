#[cfg(feature = "rock")]
mod rockusb_fwudate;
#[cfg(feature = "rock")]
use self::rockusb_fwudate::new_rockusb_transport;
#[cfg(feature = "rpi")]
mod rpi_fwupdate;
#[cfg(feature = "rpi")]
use rpi_fwupdate::new_rpi_transport;
pub mod transport;
use self::transport::FwUpdateTransport;

use super::usbboot::FlashProgress;
pub use super::usbboot::FlashingError;
use futures::future::BoxFuture;
use once_cell::sync::Lazy;
use rusb::GlobalContext;
use std::collections::HashMap;
use tokio::sync::mpsc::Sender;

#[allow(clippy::vec_init_then_push)]
pub static SUPPORTED_MSD_DEVICES: Lazy<Vec<(u16, u16)>> = Lazy::new(|| {
    let mut supported = Vec::<(u16, u16)>::new();
    #[cfg(feature = "rpi")]
    supported.push(rpi_fwupdate::VID_PID);
    supported
});

pub static SUPPORTED_DEVICES: Lazy<HashMap<(u16, u16), FactoryItemCreator>> = Lazy::new(|| {
    let mut creators = HashMap::<(u16, u16), FactoryItemCreator>::new();
    #[cfg(feature = "rpi")]
    creators.insert(
        rpi_fwupdate::VID_PID,
        Box::new(|_, logging| {
            Box::pin(async move { new_rpi_transport(&logging).await.map(Into::into) })
        }),
    );

    #[cfg(feature = "rock")]
    creators.insert(
        rockusb_fwudate::RK3588_VID_PID,
        Box::new(|device, logging| {
            let clone = device.clone();
            Box::pin(async move { new_rockusb_transport(clone, &logging).await.map(Into::into) })
        }),
    );

    creators
});

pub type FactoryItem = BoxFuture<'static, Result<Box<dyn FwUpdateTransport>, FlashingError>>;
pub type FactoryItemCreator =
    Box<dyn Fn(&rusb::Device<GlobalContext>, Sender<FlashProgress>) -> FactoryItem + Send + Sync>;

pub fn fw_update_transport(
    device: &rusb::Device<GlobalContext>,
    logging: Sender<FlashProgress>,
) -> anyhow::Result<FactoryItem> {
    let descriptor = device.device_descriptor()?;
    let vid_pid = (descriptor.vendor_id(), descriptor.product_id());

    SUPPORTED_DEVICES
        .get(&vid_pid)
        .map(|creator| creator(device, logging))
        .ok_or(anyhow::anyhow!("no driver available for {:?}", device))
}
