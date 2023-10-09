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
use bytes::BufMut;
use crc::{Crc, Digest};
use std::{
    io::{self},
    ops::Deref,
    pin::Pin,
    task::Poll,
};
use tokio::{
    io::{AsyncRead, ReadBuf},
    sync::mpsc::Receiver,
};

/// This struct wraps a [tokio::sync::mpsc::Receiver] and transforms that
/// exposes a [AsyncRead] interface.
pub struct ReceiverReader<T>
where
    T: Deref<Target = [u8]>,
{
    receiver: Receiver<T>,
    buffer: Vec<u8>,
}

impl<T> ReceiverReader<T>
where
    T: Deref<Target = [u8]>,
{
    pub fn new(receiver: Receiver<T>) -> Self {
        Self {
            receiver,
            buffer: Vec::new(),
        }
    }

    pub fn push_to_buffer(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
    }

    pub fn take_buffered_bytes(&mut self, read_buf: &mut ReadBuf<'_>) -> bool {
        let len = self.buffer.len().min(read_buf.remaining());
        if len > 0 {
            let data: Vec<u8> = self.buffer.drain(..len).collect();
            read_buf.put_slice(&data);
            true
        } else {
            false
        }
    }
}

impl<T> AsyncRead for ReceiverReader<T>
where
    T: Deref<Target = [u8]>,
{
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let mut read_bytes = this.take_buffered_bytes(buf);

        while buf.has_remaining_mut() {
            match this.receiver.poll_recv(cx) {
                Poll::Ready(Some(c)) => {
                    let bytes = c.deref();
                    let len = bytes.len().min(buf.remaining());
                    buf.put_slice(&bytes[..len]);
                    read_bytes = true;

                    if len < bytes.len() {
                        this.push_to_buffer(&bytes[len..]);
                    }
                }
                Poll::Ready(None) => {
                    if !read_bytes {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "channel closed",
                        )));
                    } else {
                        return Poll::Ready(Ok(()));
                    }
                }
                Poll::Pending => return Poll::Pending,
            };
        }

        Poll::Ready(Ok(()))
    }
}

pub struct CrcReader<'a, R>
where
    R: AsyncRead,
{
    digest: Digest<'a, u64>,
    reader: R,
}

pub fn reader_with_crc64<T: AsyncRead>(reader: T, crc: &Crc<u64>) -> CrcReader<'_, T> {
    CrcReader {
        digest: crc.digest(),
        reader,
    }
}

impl<'a, R> AsyncRead for CrcReader<'a, R>
where
    R: AsyncRead + Unpin,
{
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let me = Pin::get_mut(self);
        let result;
        let filled;
        {
            let mut buffer = buf.take(buf.remaining());
            result = Pin::new(&mut me.reader).poll_read(cx, &mut buffer);
            filled = buffer.capacity() - buffer.remaining();

            me.digest.update(buffer.filled());
        }

        buf.advance(filled);
        result
    }
}

impl<'a, R> CrcReader<'a, R>
where
    R: AsyncRead,
{
    pub fn crc(self) -> u64 {
        self.digest.finalize()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crc::CRC_64_REDIS;
    use rand::RngCore;
    use std::io::Cursor;
    use tokio::{io::AsyncReadExt, sync::mpsc::channel};

    fn random_array<const SIZE: usize>() -> Vec<u8> {
        let mut array = vec![0; SIZE];
        rand::thread_rng().fill_bytes(&mut array);
        array
    }

    #[tokio::test]
    async fn crc_reader_test() {
        let mut buffer = random_array::<{ 1024 * 1024 + 23 }>();
        let expected_crc = Crc::<u64>::new(&CRC_64_REDIS).checksum(&buffer);

        let mut data = Vec::new();
        let crc = {
            let cursor = Cursor::new(&mut buffer);
            let crc = Crc::<u64>::new(&CRC_64_REDIS);
            let mut reader = reader_with_crc64(cursor, &crc);

            let mut chunk = vec![0; 1044];
            while let Ok(read) = reader.read(&mut chunk).await {
                if read == 0 {
                    break;
                }
                data.extend_from_slice(&chunk[0..read]);
            }
            reader.crc()
        };

        assert_eq!(data, buffer);
        assert_eq!(expected_crc, crc);
    }

    #[tokio::test]
    async fn receive_once_and_drain_buffer_test() {
        let (sender, receiver) = channel::<Vec<u8>>(2);
        let mut rr = ReceiverReader::new(receiver);
        sender.send(vec![1, 2]).await.unwrap();
        drop(sender);

        let mut buffer = [0u8; 5];
        rr.read(&mut buffer[0..1]).await.unwrap();
        assert_eq!(buffer[0], 1);
        rr.read(&mut buffer[0..1]).await.unwrap();
        assert_eq!(buffer[0], 2);
        assert!(rr.read(&mut buffer[0..1]).await.is_err());
    }

    #[tokio::test]
    async fn drain_buffer_and_new_read_available_test() {
        let (sender, receiver) = channel::<Vec<u8>>(2);
        let mut rr = ReceiverReader::new(receiver);
        sender.send(vec![1, 2]).await.unwrap();

        let mut buffer = [0u8; 5];
        rr.read(&mut buffer[0..1]).await.unwrap();
        assert_eq!(buffer[0], 1);
        rr.read(&mut buffer[0..1]).await.unwrap();
        assert_eq!(buffer[0], 2);
        sender.send(vec![8, 9]).await.unwrap();
        rr.read(&mut buffer[0..2]).await.unwrap();
        assert_eq!(vec![8, 9], buffer[0..2]);
    }

    #[tokio::test]
    async fn exhaust_reader_return_result() {
        let (sender, receiver) = channel::<Vec<u8>>(2);
        let result = tokio::spawn(async {
            let mut rr = ReceiverReader::new(receiver);
            let mut buffer = [0u8; 5];
            rr.read(&mut buffer).await.unwrap();
            assert_eq!(vec![1, 2, 3, 4, 5], buffer);
            rr
        });

        sender.send(vec![1, 2]).await.unwrap();
        sender.send(vec![3, 4, 5, 6, 7]).await.unwrap();
        let mut rr = result.await.unwrap();
        drop(sender);

        let mut buffer = [0u8; 4];
        let res = rr.read(&mut buffer).await.unwrap();
        // we have exhaust the channel unable to complete the whole 4 bytes
        // read request. return the last available bytes
        assert_eq!(vec![6, 7], buffer[0..2]);
        assert_eq!(res, 2);
    }
}
