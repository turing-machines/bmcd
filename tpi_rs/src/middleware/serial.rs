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

#[derive(Debug)]
struct RingBuffer<const C: usize> {
    buf: Vec<u8>,
    idx: usize,
    len: usize,
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

    pub async fn read(&self, node: NodeId) -> String {
        let idx = node as usize;
        let handler = &self.handlers[idx];
        let buf = handler.buf.lock().await.read();

        String::from_utf8_lossy(&buf).to_string()
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

impl<const C: usize> RingBuffer<C> {
    fn new() -> Self {
        Self {
            buf: vec![0; C],
            idx: 0,
            len: 0,
        }
    }

    fn write(&mut self, data: &[u8]) {
        let remaining = C - (self.idx + self.len);
        let mut data_idx = 0;

        while data_idx < data.len() {
            let copy_len = (data.len() - data_idx).min(remaining);

            let beg = (self.idx + self.len) % C;
            let end = (self.idx + self.len + copy_len) % C;

            if beg < end {
                self.buf[beg..end].copy_from_slice(&data[data_idx..data_idx + copy_len]);
            } else {
                self.buf[beg..].copy_from_slice(&data[data_idx..data_idx + copy_len]);
            }

            self.len += copy_len;
            data_idx += copy_len;

            if self.len > C {
                self.idx = (self.idx + copy_len) % C;
                self.len = C;
            }
        }
    }

    fn read(&mut self) -> Vec<u8> {
        let to_read = self.len;
        let remaining = C - self.idx;
        let mut bytes_read = 0;
        let mut output = Vec::with_capacity(to_read);

        while bytes_read < to_read {
            let read_len = (to_read - bytes_read).min(remaining);

            output.extend_from_slice(&self.buf[self.idx..self.idx + read_len]);

            self.len -= read_len;
            bytes_read += read_len;

            self.idx = (self.idx + read_len) % C;
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::RingBuffer;

    #[test]
    fn test_simple() {
        let mut b = RingBuffer::<5>::new();
        let empty: Vec<u8> = vec![];
        assert_eq!(b.read(), empty);
        b.write(&[1, 2, 3]);
        assert_eq!(b.read(), [1, 2, 3]);
        assert_eq!(b.read(), empty);
    }

    #[test]
    fn test_exact_size() {
        let mut b = RingBuffer::<3>::new();
        b.write(&[1, 2, 3]);
        assert_eq!(b.read(), [1, 2, 3]);
        b.write(&[4, 5, 6, 7]);
        assert_eq!(b.read(), [5, 6, 7]);
    }

    #[test]
    fn test_overflow_simple() {
        let mut b = RingBuffer::<5>::new();
        b.write(&[1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(b.read(), [3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_overflow() {
        let mut b = RingBuffer::<5>::new();
        b.write(&[1, 2, 3]);
        b.write(&[4, 5, 6, 7]);
        assert_eq!(b.read(), [3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_overflow_exact() {
        let mut b = RingBuffer::<5>::new();
        b.write(&[1, 2]);
        b.write(&[3, 4, 5, 6, 7]);
        assert_eq!(b.read(), [3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_overflow_wrap() {
        let mut b = RingBuffer::<5>::new();
        b.write(&[1, 2]);
        b.write(&[3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(b.read(), [5, 6, 7, 8, 9]);
    }
}
