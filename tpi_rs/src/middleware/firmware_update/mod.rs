#[cfg(feature = "rock")]
mod rk1_fwudate;
#[cfg(feature = "rpi")]
mod rpi_fwupdate;
use super::usbboot::FlashProgress;
pub use super::usbboot::FlashingError;
use futures::future::BoxFuture;
use once_cell::sync::Lazy;
#[cfg(feature = "rock")]
use rk1_fwudate::Rk1FwUpdateDriver;
#[cfg(feature = "rpi")]
use rpi_fwupdate::RpiFwUpdate;
use rusb::GlobalContext;
use std::collections::HashMap;
use tokio::{
    io::{AsyncRead, AsyncSeek, AsyncWrite},
    sync::mpsc::Sender,
};

#[allow(clippy::vec_init_then_push)]
pub static SUPPORTED_MSD_DEVICES: Lazy<Vec<(u16, u16)>> = Lazy::new(|| {
    let mut supported = Vec::<(u16, u16)>::new();
    #[cfg(feature = "rpi")]
    supported.push(RpiFwUpdate::VID_PID);
    supported
});

pub static SUPPORTED_DEVICES: Lazy<HashMap<(u16, u16), FactoryItemCreator>> = Lazy::new(|| {
    let mut creators = HashMap::<(u16, u16), FactoryItemCreator>::new();
    #[cfg(feature = "rpi")]
    creators.insert(
        RpiFwUpdate::VID_PID,
        Box::new(|_, logging| {
            Box::pin(async move {
                RpiFwUpdate::new(logging)
                    .await
                    .map(|u| Box::new(u) as Box<dyn FwUpdate>)
            })
        }),
    );

    #[cfg(feature = "rock")]
    creators.insert(
        Rk1FwUpdateDriver::VID_PID,
        Box::new(|device, logging| {
            let clone = device.clone();
            Box::pin(async {
                Rk1FwUpdateDriver::new(clone, logging).map(|u| Box::new(u) as Box<dyn FwUpdate>)
            })
        }),
    );

    creators
});

pub trait FwUpdate: AsyncRead + AsyncWrite + AsyncSeek + std::marker::Unpin + Send {}
pub type FactoryItem = BoxFuture<'static, Result<Box<dyn FwUpdate>, FlashingError>>;
pub type FactoryItemCreator =
    Box<dyn Fn(&rusb::Device<GlobalContext>, Sender<FlashProgress>) -> FactoryItem + Send + Sync>;

pub fn fw_update_factory(
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
