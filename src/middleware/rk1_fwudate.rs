#![allow(unused_variables)]

use super::{
    firmware_update_usb::FwUpdate,
    usbboot::{FlashProgress, FlashingError},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
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
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        todo!()
    }
}

impl AsyncWrite for Rk1FwUpdateDriver {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        todo!()
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        todo!()
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        todo!()
    }
}
