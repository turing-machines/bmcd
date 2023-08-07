use super::transport::{StdFwUpdateTransport, StdTransportWrapper};
use crate::middleware::usbboot::{FlashProgress, FlashStatus, FlashingError};
use rockusb::libusb::{Transport, TransportIO};
use rusb::GlobalContext;
use tokio::sync::mpsc::Sender;

pub const RK3588_VID_PID: (u16, u16) = (0x2207, 0x350b);
pub async fn new_rockusb_transport(
    device: rusb::Device<GlobalContext>,
    logging: Sender<FlashProgress>,
) -> Result<StdTransportWrapper<TransportIO<Transport>>, FlashingError> {
    let usb_error = |e| {
        let _ = logging.try_send(FlashProgress {
            status: FlashStatus::Error(FlashingError::UsbError),
            message: format!("{}", e),
        });
        FlashingError::UsbError
    };

    let io_error = |e| {
        let _ = logging.try_send(FlashProgress {
            status: FlashStatus::Error(FlashingError::IoError),
            message: format!("{}", e),
        });
        FlashingError::IoError
    };

    let transport = Transport::from_usb_device(device.open().map_err(usb_error)?)
        .map_err(|_| FlashingError::UsbError)?;

    Ok(StdTransportWrapper::new(
        transport.into_io().map_err(io_error)?,
    ))
}

impl StdFwUpdateTransport for TransportIO<Transport> {}
