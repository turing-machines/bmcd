#[derive(Debug)]
pub struct RingBuffer<const C: usize> {
    buf: Vec<u8>,
    idx: usize,
    len: usize,
}

impl<const C: usize> RingBuffer<C> {
    pub fn write(&mut self, data: &[u8]) {
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

    pub fn read(&mut self) -> Vec<u8> {
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

impl<const C: usize> Default for RingBuffer<C> {
    fn default() -> Self {
        Self {
            buf: vec![0; C],
            idx: 0,
            len: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RingBuffer;

    #[test]
    fn test_simple() {
        let mut b = RingBuffer::<5>::default();
        let empty: Vec<u8> = vec![];
        assert_eq!(b.read(), empty);
        b.write(&[1, 2, 3]);
        assert_eq!(b.read(), [1, 2, 3]);
        assert_eq!(b.read(), empty);
    }

    #[test]
    fn test_exact_size() {
        let mut b = RingBuffer::<3>::default();
        b.write(&[1, 2, 3]);
        assert_eq!(b.read(), [1, 2, 3]);
        b.write(&[4, 5, 6, 7]);
        assert_eq!(b.read(), [5, 6, 7]);
    }

    #[test]
    fn test_overflow_simple() {
        let mut b = RingBuffer::<5>::default();
        b.write(&[1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(b.read(), [3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_overflow() {
        let mut b = RingBuffer::<5>::default();
        b.write(&[1, 2, 3]);
        b.write(&[4, 5, 6, 7]);
        assert_eq!(b.read(), [3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_overflow_exact() {
        let mut b = RingBuffer::<5>::default();
        b.write(&[1, 2]);
        b.write(&[3, 4, 5, 6, 7]);
        assert_eq!(b.read(), [3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_overflow_wrap() {
        let mut b = RingBuffer::<5>::default();
        b.write(&[1, 2]);
        b.write(&[3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(b.read(), [5, 6, 7, 8, 9]);
    }
}
