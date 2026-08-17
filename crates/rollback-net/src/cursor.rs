//! Bounds-checked little-endian reader/writer for the wire format.
//!
//! Every read is fallible. A datagram arrives from the network and may be
//! truncated, padded or hostile even after passing the HMAC check (a peer with
//! the key can still be buggy), so decoding never indexes a slice directly.

use crate::wire::WireError;

pub struct Writer(Vec<u8>);

impl Writer {
    pub fn with_capacity(n: usize) -> Self {
        Writer(Vec::with_capacity(n))
    }

    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bytes(&mut self, v: &[u8]) {
        self.0.extend_from_slice(v);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn finish(self) -> Vec<u8> {
        self.0
    }
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, at: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if self.remaining() < n {
            return Err(WireError::Truncated {
                need: n,
                have: self.remaining(),
            });
        }
        let slice = &self.bytes[self.at..self.at + n];
        self.at += n;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, WireError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, WireError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32, WireError> {
        Ok(self.u32()? as i32)
    }

    pub fn u64(&mut self) -> Result<u64, WireError> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        let b = self.take(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(b);
        Ok(a)
    }

    /// Reject trailing bytes: a well-formed packet is consumed exactly.
    pub fn finish(self) -> Result<(), WireError> {
        if self.remaining() != 0 {
            return Err(WireError::TrailingBytes(self.remaining()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_width_round_trips() {
        let mut w = Writer::with_capacity(64);
        w.u8(0x7F);
        w.u16(0xBEEF);
        w.u32(0xDEAD_BEEF);
        w.i32(-123_456);
        w.u64(0x0123_4567_89AB_CDEF);
        w.bytes(&[1, 2, 3, 4]);
        let bytes = w.finish();

        let mut r = Reader::new(&bytes);
        assert_eq!(r.u8().unwrap(), 0x7F);
        assert_eq!(r.u16().unwrap(), 0xBEEF);
        assert_eq!(r.u32().unwrap(), 0xDEAD_BEEF);
        assert_eq!(r.i32().unwrap(), -123_456);
        assert_eq!(r.u64().unwrap(), 0x0123_4567_89AB_CDEF);
        assert_eq!(r.array::<4>().unwrap(), [1, 2, 3, 4]);
        r.finish().unwrap();
    }

    #[test]
    fn reading_past_the_end_is_an_error_not_a_panic() {
        let mut r = Reader::new(&[1, 2, 3]);
        assert!(matches!(
            r.u64(),
            Err(WireError::Truncated { need: 8, have: 3 })
        ));
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut r = Reader::new(&[1, 2, 3, 4, 5]);
        r.u32().unwrap();
        assert!(matches!(r.finish(), Err(WireError::TrailingBytes(1))));
    }
}
