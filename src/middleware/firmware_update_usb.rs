use anyhow::{bail, Ok};
use futures::future::BoxFuture;
use rusb::GlobalContext;
use tokio::{
    io::{AsyncRead, AsyncSeek, AsyncWrite},
    sync::mpsc::Sender,
};

pub use super::usbboot::FlashingError;
use super::{rk1_fwudate::Rk1FwUpdateDriver, rpi_fwupdate::RpiFwUpdate, usbboot::FlashProgress};

pub const SUPPORTED_MSD_DEVICES: [(u16, u16); 1] = [RpiFwUpdate::VID_PID];
pub const SUPPORTED_DEVICES: [(u16, u16); 2] = [RpiFwUpdate::VID_PID, Rk1FwUpdateDriver::VID_PID];

pub trait FwUpdate: AsyncRead + AsyncWrite + AsyncSeek + std::marker::Unpin + Send {}

pub type FactoryItem = BoxFuture<'static, Result<Box<dyn FwUpdate>, FlashingError>>;

pub fn fw_update_factory(
    device: &rusb::Device<GlobalContext>,
    logging: Sender<FlashProgress>,
) -> anyhow::Result<FactoryItem> {
    let descriptor = device.device_descriptor()?;
    let vid_pid = (descriptor.vendor_id(), descriptor.product_id());
    match vid_pid {
        RpiFwUpdate::VID_PID => Ok(Box::pin(async {
            RpiFwUpdate::new(logging)
                .await
                .map(|u| Box::new(u) as Box<dyn FwUpdate>)
        })),
        Rk1FwUpdateDriver::VID_PID => {
            let clone = device.clone();
            Ok(Box::pin(async {
                Rk1FwUpdateDriver::new(clone, logging).map(|u| Box::new(u) as Box<dyn FwUpdate>)
            }))
        }
        _ => bail!("no driver available for {:?}", device),
    }
}
