#![allow(dead_code, unused_variables)]
use bytes::BytesMut;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{
        broadcast,
        mpsc::{self},
    },
};

pub struct SerialChannel {
    inner: (broadcast::Receiver<BytesMut>, mpsc::Sender<BytesMut>),
}

impl SerialChannel {
    pub fn new(receiver: broadcast::Receiver<BytesMut>, sender: mpsc::Sender<BytesMut>) -> Self {
        Self {
            inner: (receiver, sender),
        }
    }
}

impl AsyncRead for SerialChannel {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        todo!()
    }
}

impl AsyncWrite for SerialChannel {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::result::Result<usize, std::io::Error>> {
        todo!()
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        todo!()
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        todo!()
    }
}
