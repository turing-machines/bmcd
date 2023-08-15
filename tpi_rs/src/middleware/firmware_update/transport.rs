use core::{
    pin::Pin,
    task::{self, Poll},
};
use std::{borrow::BorrowMut, ops::DerefMut};
use std::{
    cell::RefCell,
    io::{Error, Read, Seek, Write},
    sync::Arc,
};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncSeek, AsyncWrite, ReadBuf},
};
pub trait FwUpdateTransport: AsyncRead + AsyncWrite + AsyncSeek + Send + Unpin {}

pub trait StdFwUpdateTransport: Read + Write + Seek + Send + Unpin {}

impl FwUpdateTransport for File {}

impl<T: FwUpdateTransport> FwUpdateTransport for Box<T> {}

impl<'a, T: FwUpdateTransport + 'a> From<T> for Box<dyn FwUpdateTransport + 'a> {
    fn from(value: T) -> Self {
        Box::new(value) as Box<dyn FwUpdateTransport>
    }
}

pub struct StdTransportWrapper<T> {
    transport: T,
    read_buf: RefCell<Vec<u8>>,
}

impl<T: StdFwUpdateTransport> StdTransportWrapper<T> {
    pub fn new(object: T) -> Self {
        Self {
            transport: object,
            read_buf: RefCell::new(Vec::new()),
        }
    }
}

impl<T: StdFwUpdateTransport> FwUpdateTransport for StdTransportWrapper<T> {}

impl<T: StdFwUpdateTransport> AsyncRead for StdTransportWrapper<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let buffer_size = buf.capacity();
        let this = self.get_mut();
        if let Ok(mut read_buffer) = this.read_buf.try_borrow_mut() {
            let self_buffer_size = read_buffer.capacity();
            read_buffer.resize(buffer_size.max(self_buffer_size), 0u8);
            let slice = &mut read_buffer[0..buffer_size];
            this.transport.read_exact(slice)?;
            buf.put_slice(&slice);
        } else {
            let mut buffer = vec![0u8; buffer_size];
            this.transport.read_exact(&mut buffer)?;
            buf.put_slice(&buffer);
        }
        Poll::Ready(Ok(()))
    }
}

impl<T: StdFwUpdateTransport> AsyncWrite for StdTransportWrapper<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        let result = self.get_mut().transport.write(buf);
        Poll::Ready(result)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<Result<(), Error>> {
        let result = self.get_mut().transport.flush();
        Poll::Ready(result)
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<Result<(), Error>> {
        Poll::Ready(Ok(()))
    }
}

impl<T: StdFwUpdateTransport> AsyncSeek for StdTransportWrapper<T> {
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
