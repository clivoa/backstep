//! A tiny deterministic PRNG.
//!
//! Used by the arena bot and by the network emulator. It is deliberately *not*
//! used by the arena simulation itself: the arena is fully deterministic from
//! inputs alone, which is what makes the 100k-frame replay test meaningful.
//!
//! Algorithm is xorshift64*, chosen because it is a handful of integer ops with
//! no platform-dependent behaviour whatsoever.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    pub const fn new(seed: u64) -> Self {
        // A zero state is a fixed point of xorshift, so fold it away.
        Self {
            state: if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform value in `[0, bound)`. Returns 0 when `bound` is 0.
    pub fn below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        // Modulo bias is irrelevant at the scales used here (bounds < 10_000)
        // and rejection sampling would make the stream length input-dependent,
        // which is worse for reproducibility.
        self.next_u32() % bound
    }

    /// True with probability `permille / 1000`.
    pub fn chance_permille(&mut self, permille: u32) -> bool {
        self.below(1000) < permille
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let mut a = DeterministicRng::new(42);
        let mut b = DeterministicRng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn zero_seed_still_advances() {
        let mut r = DeterministicRng::new(0);
        let first = r.next_u64();
        assert_ne!(first, 0);
        assert_ne!(first, r.next_u64());
    }

    #[test]
    fn below_respects_bound() {
        let mut r = DeterministicRng::new(7);
        for _ in 0..10_000 {
            assert!(r.below(13) < 13);
        }
        assert_eq!(r.below(0), 0);
    }

    #[test]
    fn chance_permille_endpoints_are_absolute() {
        let mut r = DeterministicRng::new(99);
        for _ in 0..1000 {
            assert!(!r.chance_permille(0));
            assert!(r.chance_permille(1000));
        }
    }
}
