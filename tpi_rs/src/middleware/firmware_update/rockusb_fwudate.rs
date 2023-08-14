use super::transport::{StdFwUpdateTransport, StdTransportWrapper};
use crate::middleware::usbboot::{FlashProgress, FlashingError, FlashingErrorExt};
use rockusb::libusb::{Transport, TransportIO};
use rusb::GlobalContext;
use tokio::sync::mpsc::Sender;

pub const RK3588_VID_PID: (u16, u16) = (0x2207, 0x350b);
pub async fn new_rockusb_transport(
    device: rusb::Device<GlobalContext>,
    logging: &Sender<FlashProgress>,
) -> Result<StdTransportWrapper<TransportIO<Transport>>, FlashingError> {
    let transport = Transport::from_usb_device(device.open().map_err_into_logged_usb(logging)?)
        .map_err(|_| FlashingError::UsbError)?;

    Ok(StdTransportWrapper::new(
        transport.into_io().map_err_into_logged_io(logging)?,
    ))
}

impl StdFwUpdateTransport for TransportIO<Transport> {}
