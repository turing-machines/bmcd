#![allow(unused_variables, dead_code)]
use crate::middleware::usbboot::FlashStatus;

use super::{
    firmware_update_usb::FwUpdate,
    usbboot::{FlashProgress, FlashingError},
};
use core::{
    pin::Pin,
    task::{self, Poll},
};
use rockusb::libusb::{Devices, Transport};
use std::io::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::mpsc::Sender,
};

pub struct Rk1FwUpdateDriver {
    transport: Transport,
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

        Ok(Self { transport })
    }
}

impl FwUpdate for Rk1FwUpdateDriver {}

impl AsyncRead for Rk1FwUpdateDriver {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        todo!()
    }
}

impl AsyncWrite for Rk1FwUpdateDriver {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        todo!()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Result<(), Error>> {
        todo!()
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Result<(), Error>> {
        todo!()
    }
}
