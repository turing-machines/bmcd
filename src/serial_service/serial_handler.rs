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
use bytes::Bytes;
use circular_buffer::CircularBuffer;
use futures::StreamExt;
use futures::{Sink, SinkExt, Stream};
use log::debug;
use serde::Serialize;
use std::io::{self, ErrorKind, Write};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{
    broadcast,
    mpsc::{self, error::SendError, WeakSender},
    Mutex,
};
use tokio_serial::{DataBits, Parity, SerialPortBuilderExt, StopBits};
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::codec::{BytesCodec, Decoder};
use tokio_util::sync::PollSender;

use crate::utils::{string_from_utf16, string_from_utf32};

type RingBuffer = CircularBuffer<OUTPUT_BUF_SIZE, u8>;

const OUTPUT_BUF_SIZE: usize = 16 * 1024;

/// A [`Handler`] is an object that controls a single UART connection from the
/// BMC to a predefined node. It has 2 functions to directly read and write the
/// serial, see [`Self::write`] and [`Self::read_whole_buffer`]. However its
/// recommended to open a channel using the [`Self::open_channel`] method. A
/// [`Handler`] can handle an arbitrary amount of channels which read/write to
/// the UART port.
#[derive(Debug)]
pub struct Handler {
    node: usize,
    baud_rate: u32,
    data_bits: DataBits,
    parity: Parity,
    stop_bits: StopBits,
    path: &'static str,
    ring_buffer: Arc<Mutex<Box<RingBuffer>>>,
    worker_context: Option<(broadcast::Sender<Bytes>, mpsc::Sender<Bytes>)>,
    writer: Option<WeakSender<Bytes>>,
}

impl Handler {
    pub fn new(
        node: usize,
        path: &'static str,
        baud_rate: u32,
        data_bits: DataBits,
        parity: Parity,
        stop_bits: StopBits,
    ) -> Self {
        Handler {
            node,
            path,
            baud_rate,
            data_bits,
            parity,
            stop_bits,
            ring_buffer: Arc::new(Mutex::new(RingBuffer::boxed())),
            worker_context: None,
            writer: None,
        }
    }

    /// Returns the current state of the Handler.
    pub fn get_state(&self) -> HandlerState {
        match &self.worker_context {
            Some((_, sender)) if sender.is_closed() => HandlerState::Stopped,
            Some(_) => HandlerState::Running,
            None => HandlerState::Initialized,
        }
    }

    /// Opens a bi-directional asynchronous data-stream which can be used to
    /// read and write bytes from and to the serial port.
    ///
    /// # Returns
    ///
    /// * `SerialError::NotStarted` when [`Self::run`] was not called
    /// successfully
    #[allow(dead_code)]
    pub fn open_channel(
        &self,
    ) -> Result<
        (
            impl Stream<Item = io::Result<Bytes>>,
            impl Sink<bytes::Bytes, Error = io::Error>,
        ),
        SerialError,
    > {
        let Some((read_sender, write_sender)) = &self.worker_context else {
            return Err(SerialError::NotStarted);
        };

        let poll_sender = PollSender::new(write_sender.clone())
            .sink_map_err(|_| io::Error::from(ErrorKind::BrokenPipe));
        let stream = BroadcastStream::new(read_sender.subscribe())
            .map(|res| res.map_err(|e| io::Error::new(ErrorKind::BrokenPipe, e)));
        Ok((stream, poll_sender))
    }

    /// Reads the whole circular buffer encoded as a String object
    pub async fn read_as_string(&self, encoding: Encoding) -> Result<String, SerialError> {
        let bytes = self.read_whole_buffer().await?;
        let res = match encoding {
            Encoding::Utf8 => String::from_utf8_lossy(&bytes).to_string(),
            Encoding::Utf16 { little_endian } => string_from_utf16(&bytes, little_endian),
            Encoding::Utf32 { little_endian } => string_from_utf32(&bytes, little_endian),
        };
        Ok(res)
    }

    /// This function returns all the cached data.
    /// Time complexity: O(N)
    pub async fn read_whole_buffer(&self) -> Result<Bytes, SerialError> {
        if self.worker_context.is_none() {
            return Err(SerialError::NotStarted);
        };

        let mut rb = self.ring_buffer.lock().await;
        debug!("reading {} from node {}", rb.len(), self.node);
        Ok(Bytes::copy_from_slice(rb.make_contiguous()))
    }

    /// Serial write interface.
    ///
    /// # Returns
    ///
    /// * `SerialError::NotStarted` when [`Self::run`] was not called
    /// successfully.
    /// * `SerialError::Stopped` when the handler is not running anymore.
    ///
    pub async fn write(&self, bytes: Bytes) -> Result<(), SerialError> {
        let writer = self
            .writer
            .as_ref()
            .ok_or(SerialError::NotStarted)?
            .upgrade()
            .ok_or(SerialError::Stopped)?;

        writer.send(bytes).await?;
        Ok(())
    }

    pub fn run(&mut self) -> Result<(), SerialError> {
        if self.worker_context.take().is_some() {
            return Err(SerialError::AlreadyRunning);
        };

        let mut port = tokio_serial::new(self.path, self.baud_rate)
            .data_bits(self.data_bits)
            .parity(self.parity)
            .stop_bits(self.stop_bits)
            .open_native_async()?;

        // Disable exclusivity of the port to allow other applications to open it.
        // Not a reason to abort if we can't.
        if let Err(e) = port.set_exclusive(false) {
            log::warn!("Unable to set exclusivity of port {}: {}", self.path, e);
        }

        let (read_sender, _) = broadcast::channel::<Bytes>(8);
        let (write_sender, mut write_receiver) = mpsc::channel::<Bytes>(8);
        self.worker_context = Some((read_sender.clone(), write_sender.clone()));

        let node = self.node;
        let buffer = self.ring_buffer.clone();
        tokio::spawn(async move {
            log::info!("[node {}] serial started", &node);
            let (mut sink, mut stream) = BytesCodec::new().framed(port).split();
            loop {
                tokio::select! {
                    res = write_receiver.recv() => {
                        let Some(data) = res else {
                            log::error!("error sending data to uart");
                            break;
                        };

                        if let Err(e) = sink.send(data).await {
                            log::error!("{}", e);
                        }
                    },
                    res = stream.next() => {
                        let Some(res) = res else {
                            log::error!("Error reading serial stream of node {}", node);
                            break;
                        };

                        let Ok(bytes) = res else {
                            log::error!("Serial stream of node {} has closed", node);
                            break;
                        };

                        debug!("writing {} bytes node {}", bytes.len(), node);
                        // Implementation is actually infallible in the currently used v0.1.3
                        if buffer.lock().await.write(&bytes).is_err() {
                            log::error!("Failed to write to buffer of node {}", node);
                            break;
                        };

                        if read_sender.receiver_count() > 0 {
                            if let Err(e) = read_sender.send(bytes.into()) {
                                log::error!("broadcast error: {:#}", e);
                                break;
                            }
                        }
                    },
                }
            }
            log::warn!("exiting serial worker for node {} ", &node);
        });

        self.writer = Some(write_sender.downgrade());
        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum SerialError {
    #[error("serial worker not started")]
    NotStarted,
    #[error("already running")]
    AlreadyRunning,
    #[error("Stopped")]
    Stopped,
    #[error(transparent)]
    SendError(#[from] SendError<bytes::Bytes>),
    #[error(transparent)]
    TokioSerial(#[from] tokio_serial::Error),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

#[derive(Serialize)]
pub enum HandlerState {
    Initialized,
    Running,
    Stopped,
}

/// Encodings used when reading from a serial port
pub enum Encoding {
    Utf8,
    Utf16 { little_endian: bool },
    Utf32 { little_endian: bool },
}
