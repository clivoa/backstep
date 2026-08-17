//! A byte cursor for the arena's fixed-layout state blob.
//!
//! Everything is little-endian `i32`. Nothing is length-prefixed or optional:
//! the blob is a fixed number of bytes and a wrong length is an error rather
//! than something to recover from, because a state blob only ever comes from
//! this crate's own `save_state`.

use rollback_core::SimulationError;

pub struct Writer(Vec<u8>);

impl Writer {
    pub fn with_capacity(bytes: usize) -> Self {
        Writer(Vec::with_capacity(bytes))
    }

    pub fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bool(&mut self, v: bool) {
        self.u32(u32::from(v));
    }

    pub fn finish(self) -> Vec<u8> {
        self.0
    }
}

#[derive(Debug)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    /// Fail early if the blob is not exactly `expected` bytes long.
    pub fn new(bytes: &'a [u8], expected: usize) -> Result<Self, SimulationError> {
        if bytes.len() != expected {
            return Err(SimulationError::StateSize {
                expected,
                actual: bytes.len(),
            });
        }
        Ok(Reader { bytes, at: 0 })
    }

    pub fn i32(&mut self) -> i32 {
        let mut b = [0u8; 4];
        b.copy_from_slice(&self.bytes[self.at..self.at + 4]);
        self.at += 4;
        i32::from_le_bytes(b)
    }

    pub fn u32(&mut self) -> u32 {
        self.i32() as u32
    }

    pub fn bool(&mut self) -> bool {
        self.u32() != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_in_order() {
        let mut w = Writer::with_capacity(16);
        w.i32(-42);
        w.u32(0xDEAD_BEEF);
        w.bool(true);
        w.bool(false);
        let bytes = w.finish();
        assert_eq!(bytes.len(), 16);

        let mut r = Reader::new(&bytes, 16).unwrap();
        assert_eq!(r.i32(), -42);
        assert_eq!(r.u32(), 0xDEAD_BEEF);
        assert!(r.bool());
        assert!(!r.bool());
    }

    #[test]
    fn a_wrong_length_is_refused_before_any_read() {
        let err = Reader::new(&[0u8; 7], 8).unwrap_err();
        assert!(matches!(
            err,
            SimulationError::StateSize {
                expected: 8,
                actual: 7
            }
        ));
    }
}
