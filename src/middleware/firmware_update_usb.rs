use futures::future::BoxFuture;
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
    vid_pid: (u16, u16),
    logging: Sender<FlashProgress>,
) -> Option<FactoryItem> {
    if vid_pid == RpiFwUpdate::VID_PID {
        Some(Box::pin(async {
            RpiFwUpdate::new(logging)
                .await
                .map(|u| Box::new(u) as Box<dyn FwUpdate>)
        }))
    } else if vid_pid == Rk1FwUpdateDriver::VID_PID {
        Some(Box::pin(async {
            Rk1FwUpdateDriver::new(logging).map(|u| Box::new(u) as Box<dyn FwUpdate>)
        }))
    } else {
        None
    }
}
