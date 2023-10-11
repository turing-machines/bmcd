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
use crc::{Crc, Digest};
use std::pin::Pin;
use tokio::io::AsyncRead;

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
    use tokio::io::AsyncReadExt;

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
}
