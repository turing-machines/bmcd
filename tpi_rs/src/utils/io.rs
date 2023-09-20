use bytes::BufMut;
use std::{io, ops::Deref, task::Poll};
use tokio::{
    io::{AsyncRead, ReadBuf},
    sync::mpsc::Receiver,
};

/// This struct wraps a [tokio::sync::mpsc::Receiver] and transforms that
/// exposes a [AsyncRead] interface.
///
/// # Cancel
///
/// This struct is *not* cancel safe! Using this struct in a tokio::select loop
/// can cause data loss.
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

#[cfg(test)]
mod test {
    use super::*;
    use tokio::{io::AsyncReadExt, sync::mpsc::channel};

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
