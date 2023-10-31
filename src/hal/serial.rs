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
//! Handlers for UART connections to/from nodes
use std::error::Error;
use std::fmt::Display;
use std::sync::Arc;

use anyhow::Result;
use bytes::{Bytes, BytesMut};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc::{channel, Sender};
use tokio::sync::Mutex;
use tokio_serial::{DataBits, Parity, SerialPortBuilderExt, StopBits};
use tokio_util::codec::{BytesCodec, Decoder};

use crate::utils::ring_buf::RingBuffer;

use super::NodeId;

const OUTPUT_BUF_SIZE: usize = 16 * 1024;

#[derive(Debug)]
pub struct SerialConnections {
    handlers: Vec<Mutex<Handler>>,
}

impl SerialConnections {
    pub fn new() -> Result<Self> {
        let paths = ["/dev/ttyS2", "/dev/ttyS1", "/dev/ttyS4", "/dev/ttyS5"];

        let handlers: Vec<Mutex<Handler>> = paths
            .iter()
            .enumerate()
            .map(|(i, path)| Mutex::new(Handler::new(i + 1, path)))
            .collect();

        Ok(SerialConnections { handlers })
    }

    pub async fn run(&self) -> Result<(), SerialError> {
        for h in &self.handlers {
            h.lock().await.start_reader()?;
        }
        Ok(())
    }

    pub async fn read(&self, node: NodeId) -> Result<Bytes, SerialError> {
        let idx = node as usize;
        self.handlers[idx].lock().await.read().await
    }

    pub async fn write<B: Into<BytesMut>>(&self, node: NodeId, data: B) -> Result<(), SerialError> {
        let idx = node as usize;
        self.handlers[idx].lock().await.write(data.into()).await
    }
}

#[derive(Debug)]
struct Handler {
    node: usize,
    path: &'static str,
    ring_buffer: Arc<Mutex<RingBuffer<OUTPUT_BUF_SIZE>>>,
    worker_context: Option<Sender<BytesMut>>,
}

impl Handler {
    fn new(node: usize, path: &'static str) -> Self {
        Handler {
            node,
            path,
            ring_buffer: Arc::new(Mutex::new(RingBuffer::default())),
            worker_context: None,
        }
    }

    async fn write<B: Into<BytesMut>>(&self, data: B) -> Result<(), SerialError> {
        let Some(sender) = &self.worker_context else {
            return Err(SerialError::NotStarted);
        };

        sender
            .send(data.into())
            .await
            .map_err(|e| SerialError::InternalError(e.to_string()))
    }

    async fn read(&self) -> Result<Bytes, SerialError> {
        if self.worker_context.is_none() {
            return Err(SerialError::NotStarted);
        };

        Ok(self.ring_buffer.lock().await.read().into())
    }

    fn start_reader(&mut self) -> Result<(), SerialError> {
        if self.worker_context.take().is_some() {
            return Err(SerialError::AlreadyRunning);
        };

        let baud_rate = 115200;
        let mut port = tokio_serial::new(self.path, baud_rate)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(StopBits::One)
            .open_native_async()
            .map_err(|e| SerialError::InternalError(e.to_string()))?;

        // Disable exclusivity of the port to allow other applications to open it.
        // Not a reason to abort if we can't.
        if let Err(e) = port.set_exclusive(false) {
            log::warn!("Unable to set exclusivity of port {}: {}", self.path, e);
        }

        let (sender, mut receiver) = channel::<BytesMut>(64);
        self.worker_context = Some(sender);

        let node = self.node;
        let buffer = self.ring_buffer.clone();
        tokio::spawn(async move {
            let (mut sink, mut stream) = BytesCodec::new().framed(port).split();
            loop {
                tokio::select! {
                    res = receiver.recv() => {
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
                        buffer.lock().await.write(&bytes);

                    },
                }
            }
            log::warn!("exiting serial worker");
        });

        Ok(())
    }
}

#[derive(Debug)]
pub enum SerialError {
    NotStarted,
    AlreadyRunning,
    InternalError(String),
}

impl Error for SerialError {}

impl Display for SerialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SerialError::NotStarted => write!(f, "serial worker not started"),
            SerialError::AlreadyRunning => write!(f, "already running"),
            SerialError::InternalError(e) => e.fmt(f),
        }
    }
}
