//! Handlers for UART connections to/from nodes

use std::sync::Arc;

use anyhow::Result;
use bytes::BytesMut;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_serial::{DataBits, Parity, SerialPortBuilderExt, SerialStream, StopBits};
use tokio_util::codec::{BytesCodec, Decoder, Framed};

use crate::utils::ring_buf::RingBuffer;

use super::NodeId;

const OUTPUT_BUF_SIZE: usize = 16 * 1024;

type Sink = SplitSink<Framed<SerialStream, BytesCodec>, BytesMut>;
type Stream = SplitStream<Framed<SerialStream, BytesCodec>>;

#[derive(Debug)]
pub struct SerialConnections {
    handlers: Vec<Handler>,
}

#[derive(Debug)]
struct Handler {
    node: usize,
    sink: Sink,
    stream: Arc<Mutex<Stream>>,
    buf: Arc<Mutex<RingBuffer<OUTPUT_BUF_SIZE>>>,
    worker: Option<JoinHandle<()>>,
}

impl SerialConnections {
    pub fn new() -> Result<Self> {
        let paths = ["/dev/ttyS2", "/dev/ttyS1", "/dev/ttyS4", "/dev/ttyS5"];
        let mut handlers = vec![];

        for (i, path) in paths.iter().enumerate() {
            handlers.push(Handler::new(i + 1, path)?);
        }

        Ok(SerialConnections { handlers })
    }

    pub fn run(&mut self) {
        for h in &mut self.handlers {
            h.start_reader();
        }
    }

    pub async fn read(&self, node: NodeId) -> Vec<u8> {
        let idx = node as usize;
        let handler = &self.handlers[idx];

        handler.buf.lock().await.read()
    }

    pub async fn write(&mut self, node: NodeId, data: &[u8]) -> Result<()> {
        let idx = node as usize;

        self.handlers[idx].write(data).await
    }
}

impl Handler {
    fn new(node: usize, path: &'static str) -> Result<Self> {
        let baud_rate = 115200;
        let mut port = tokio_serial::new(path, baud_rate)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(StopBits::One)
            .open_native_async()?;

        // Disable exclusivity of the port to allow other applications to open it.
        // Not a reason to abort if we can't.
        if let Err(e) = port.set_exclusive(false) {
            log::warn!("Unable to set exclusivity of port {}: {}", path, e);
        }

        let (sink, stream) = BytesCodec::new().framed(port).split();
        let buf = RingBuffer::new();

        let stream = Arc::new(Mutex::new(stream));
        let buf = Arc::new(Mutex::new(buf));

        let worker = None;

        let inst = Handler {
            node,
            sink,
            stream,
            buf,
            worker,
        };

        Ok(inst)
    }

    async fn write(&mut self, data: &[u8]) -> Result<()> {
        let bytes = BytesMut::from(data);

        self.sink.send(bytes).await?;

        Ok(())
    }

    fn start_reader(&mut self) {
        let buf = self.buf.clone();
        let stream = self.stream.clone();
        let node = self.node;

        let worker = tokio::spawn(async move {
            loop {
                let Some(res) = stream.lock().await.next().await else {
                    log::warn!("Error reading serial stream of node {}", node);
                    break;
                };

                let Ok(bytes) = res else {
                    log::warn!("Serial stream of node {} has closed", node);
                    break;
                };

                let mut buf = buf.lock().await;
                buf.write(bytes.as_ref());
            }
        });

        self.worker = Some(worker);
    }
}
