use crate::middleware::usbboot::FlashStatus;

use super::{
    firmware_update_usb::FwUpdate,
    usbboot::{FlashProgress, FlashingError},
};
use bytes::BufMut;
use core::{
    pin::Pin,
    task::{self, Poll},
};
use rockusb::libusb::{Devices, Transport, TransportIO};
use std::io::{Error, Seek, Write};
use tokio::{
    io::{AsyncRead, AsyncSeek, AsyncWrite, ReadBuf},
    sync::mpsc::Sender,
};

pub struct Rk1FwUpdateDriver {
    transport: TransportIO<Transport>,
}

impl Rk1FwUpdateDriver {
    pub const VID_PID: (u16, u16) = (0x2207, 0x350b);
    pub fn new(logging: Sender<FlashProgress>) -> Result<Self, FlashingError> {
        let dev_not_found = || {
            let _ = logging.try_send(FlashProgress {
                status: FlashStatus::Error(FlashingError::DeviceNotFound),
                message: "could not find a connected RK1".to_string(),
            });
            FlashingError::DeviceNotFound
        };

        let io_error = |e| {
            let _ = logging.try_send(FlashProgress {
                status: FlashStatus::Error(FlashingError::IoError),
                message: format!("{}", e),
            });
            FlashingError::IoError
        };

        let devices = Devices::new().map_err(|_| FlashingError::IoError)?;
        let mut transport = devices
            .iter()
            .next()
            .ok_or_else(dev_not_found)?
            .map_err(|_| FlashingError::UsbError)?;

        let _ = logging.try_send(FlashProgress {
            status: FlashStatus::Setup,
            message: format!(
                "Chip Info: {:0x?}",
                transport.chip_info().map_err(|_| FlashingError::UsbError)?
            ),
        });

        Ok(Self {
            transport: transport.into_io().map_err(io_error)?,
        })
    }
}

impl FwUpdate for Rk1FwUpdateDriver {}

impl AsyncRead for Rk1FwUpdateDriver {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        //TODO: upgrade implementation to an async variant.
        let mut writer = buf.writer();
        let this = self.get_mut();
        let result = std::io::copy(&mut this.transport, &mut writer).map(|_| ());
        Poll::Ready(result)
    }
}

impl AsyncWrite for Rk1FwUpdateDriver {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        //TODO: upgrade implementation to an async variant.
        let result = self.get_mut().transport.write(buf);
        Poll::Ready(result)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<Result<(), Error>> {
        //TODO: upgrade implementation to an async variant.
        let result = self.get_mut().transport.flush();
        Poll::Ready(result)
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<Result<(), Error>> {
        //TODO: upgrade implementation to an async variant.
        Poll::Ready(Ok(()))
    }
}

impl AsyncSeek for Rk1FwUpdateDriver {
    fn start_seek(self: Pin<&mut Self>, position: std::io::SeekFrom) -> std::io::Result<()> {
        self.get_mut().transport.seek(position).map(|_| ())
    }

    fn poll_complete(
        self: Pin<&mut Self>,
        _cx: &mut task::Context<'_>,
    ) -> Poll<std::io::Result<u64>> {
        let pos = self.get_mut().transport.stream_position()?;
        Poll::Ready(Ok(pos))
    }
}
