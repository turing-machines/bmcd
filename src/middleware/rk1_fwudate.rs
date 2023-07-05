#![allow(unused_variables)]
use super::{
    firmware_update_usb::FwUpdate,
    usbboot::{FlashProgress, FlashingError},
};
use core::{
    pin::Pin,
    task::{Context, Poll},
};
use std::io::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::mpsc::Sender,
};

#[derive(Debug)]
pub struct Rk1FwUpdateDriver {}

impl Rk1FwUpdateDriver {
    pub const VID_PID: (u16, u16) = (0x2207, 0x350b);
    pub fn new(logging: Sender<FlashProgress>) -> Result<Self, FlashingError> {
        todo!()
    }
}

impl FwUpdate for Rk1FwUpdateDriver {}

impl AsyncRead for Rk1FwUpdateDriver {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        todo!()
    }
}

impl AsyncWrite for Rk1FwUpdateDriver {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        todo!()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        todo!()
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        todo!()
    }
}
