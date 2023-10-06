// Copyright 2023 Turing Machines
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
use core::{
    pin::Pin,
    task::{self, Poll},
};
use std::{
    cell::RefCell,
    io::{Error, Read, Seek, Write},
};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncSeek, AsyncWrite, ReadBuf},
};

/// Trait that specifies a transport object that can be used to send raw firmware images over.
pub trait FwUpdateTransport: AsyncRead + AsyncWrite + AsyncSeek + Send + Unpin {}

/// Blocking trait that specifies a transport object used to send raw firmware
/// images over. When possible use the async [FwUpdateTransport] trait.
/// [StdFwUpdateTransport] needs to be wrapped by a [StdTransportWrapper] in
/// order to be used by the firmware_application
pub trait StdFwUpdateTransport: Read + Write + Seek + Send + Unpin {}

impl FwUpdateTransport for File {}
impl<T: FwUpdateTransport> FwUpdateTransport for Box<T> {}
impl<'a, T: FwUpdateTransport + 'a> From<T> for Box<dyn FwUpdateTransport + 'a> {
    fn from(value: T) -> Self {
        Box::new(value) as Box<dyn FwUpdateTransport>
    }
}

/// A wrapper that exposes a [StdTransportWrapper] as a [FwUpdateTransport]
/// object. Used to make blocking transport compatible with our
/// firmware_application.
pub struct StdTransportWrapper<T> {
    transport: T,
    buffer: RefCell<Vec<u8>>,
}

impl<T: StdFwUpdateTransport> StdTransportWrapper<T> {
    pub fn new(object: T) -> Self {
        Self {
            transport: object,
            buffer: RefCell::new(Vec::new()),
        }
    }
}

impl<T: StdFwUpdateTransport> FwUpdateTransport for StdTransportWrapper<T> {}

impl<T: StdFwUpdateTransport> AsyncRead for StdTransportWrapper<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut task::Context<'_>,
        read_buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let required_buffer_size = read_buffer.capacity();
        let mut buffer = this.buffer.borrow_mut();
        let buffer_size = buffer.capacity();
        buffer.resize(buffer_size.max(required_buffer_size), 0u8);
        let slice = &mut buffer[0..required_buffer_size];
        this.transport.read_exact(slice)?;
        read_buffer.put_slice(slice);
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
