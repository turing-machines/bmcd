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
use crc::{Crc, Digest as CrcDigest};
use futures::Stream;
use sha2::{Digest, Sha256};
use std::{io, pin::Pin, task::Poll};
use tokio::{io::AsyncWrite, sync::watch};

pub struct Sha256StreamValidator<T>
where
    T: Stream<Item = bytes::Bytes>,
{
    hasher: Option<Sha256>,
    expected_sha: bytes::Bytes,
    stream: T,
}

impl<T> Sha256StreamValidator<T>
where
    T: Stream<Item = bytes::Bytes>,
{
    pub fn new(stream: T, expected_sha: bytes::Bytes) -> Self {
        Self {
            stream,
            expected_sha,
            hasher: Some(Sha256::new()),
        }
    }

    pub fn verify_hash(&mut self) -> io::Result<()> {
        let sha256 = self
            .hasher
            .replace(Sha256::new())
            .expect("hasher cannot be None")
            .finalize()
            .to_vec();

        if sha256 != self.expected_sha {
            let got = hex::encode(sha256);
            let expected = hex::encode(&self.expected_sha);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("sha256 checksum failed. Expected:{}, got:{}", expected, got),
            ));
        }

        Ok(())
    }
}

impl<T> Stream for Sha256StreamValidator<T>
where
    T: Stream<Item = bytes::Bytes> + Unpin,
{
    type Item = io::Result<bytes::Bytes>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let me = Pin::get_mut(self);
        let result = Pin::new(&mut me.stream).poll_next(cx);
        match result {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(bytes)) => {
                me.hasher
                    .as_mut()
                    .expect("hasher can never be None")
                    .update(&bytes);
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(None) => {
                // stream is exhausted, no more bytes are incoming.
                if let Err(e) = me.verify_hash() {
                    Poll::Ready(Some(Err(e)))
                } else {
                    Poll::Ready(None)
                }
            }
        }
    }
}

pub struct WriteMonitor<'a, W>
where
    W: AsyncWrite,
{
    written: u64,
    sender: &'a mut watch::Sender<u64>,
    digest: CrcDigest<'a, u64>,
    inner: W,
}

impl<'a, W> WriteMonitor<'a, W>
where
    W: AsyncWrite,
{
    pub fn new(writer: W, sender: &'a mut watch::Sender<u64>, crc: &'a Crc<u64>) -> Self {
        Self {
            written: 0,
            sender,
            digest: crc.digest(),
            inner: writer,
        }
    }

    pub fn crc(self) -> u64 {
        self.digest.finalize()
    }
}

impl<'a, W> AsyncWrite for WriteMonitor<'a, W>
where
    W: AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let me = Pin::get_mut(self);

        let result = Pin::new(&mut me.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(written)) = result {
            me.digest.update(&buf[..written]);
            me.written += written as u64;
            me.sender.send_replace(me.written);
        }
        result
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let me = Pin::get_mut(self);
        Pin::new(&mut me.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let me = Pin::get_mut(self);
        Pin::new(&mut me.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crc::CRC_64_REDIS;
    use rand::RngCore;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn random_array<const SIZE: usize>() -> Vec<u8> {
        let mut array = vec![0; SIZE];
        rand::thread_rng().fill_bytes(&mut array);
        array
    }

    #[tokio::test]
    async fn write_watcher_test() {
        let crc = Crc::<u64>::new(&CRC_64_REDIS);
        let mut reader = tokio::io::repeat(0b101).take(1044 * 1004);
        let (mut sender, receiver) = watch::channel(0u64);
        let mut writer = WriteMonitor::new(tokio::io::sink(), &mut sender, &crc);
        let copied = tokio::io::copy(&mut reader, &mut writer).await.unwrap();
        assert_eq!(copied, 1044 * 1004);
        assert_eq!(*receiver.borrow(), 1044 * 1004);
    }

    #[tokio::test]
    async fn crc_reader_test() {
        let read_buffer = random_array::<{ 1024 * 1024 + 23 }>();
        let expected_crc = Crc::<u64>::new(&CRC_64_REDIS).checksum(&read_buffer);

        let mut data = Vec::new();
        let crc = Crc::<u64>::new(&CRC_64_REDIS);
        let (mut sender, _) = watch::channel(0u64);
        let mut writer = WriteMonitor::new(&mut data, &mut sender, &crc);

        let mut total_read = 0;
        while total_read < read_buffer.len() {
            let range = (total_read + 1044).min(read_buffer.len());
            if let Ok(write) = writer.write(&read_buffer[total_read..range]).await {
                if write == 0 {
                    break;
                }
                total_read += write;
            }
        }

        assert_eq!(expected_crc, writer.crc());
    }

    //   #[tokio::test]
    //   async fn sha256_reader_test() {
    //       let mut buffer = random_array::<{ 1024 * 1024 + 23 }>();
    //       let mut hasher = Sha256::new();
    //       hasher.update(&buffer);
    //       let expected_sha = hasher.finalize().to_vec();
    //       let cursor = Cursor::new(&mut buffer);
    //       let mut reader = Sha256StreamValidator::new(ReaderStream::new(cursor), expected_sha.into());
    //       let mut data = Vec::new();
    //       let mut chunk = vec![0; 1044];
    //       while let Ok(read) = reader.read(&mut chunk).await {
    //           if read == 0 {
    //               break;
    //           }
    //           data.extend_from_slice(&chunk[0..read]);
    //       }

    //       assert_eq!(data, buffer);
    //   }
}
