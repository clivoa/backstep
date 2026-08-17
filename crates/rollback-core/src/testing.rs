//! A minimal simulation used by tests across the workspace.
//!
//! It is intentionally trivial but *order sensitive*: the accumulator mixes the
//! frame number into every update, so replaying the same inputs in a different
//! order produces a different checksum. A simulation that merely summed inputs
//! would pass rollback tests it should fail.

use crate::input::PlayerInput;
use crate::simulation::{OutputMode, Simulation, SimulationError};

const STATE_BYTES: usize = 8 * 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CounterSim {
    pub frame: u64,
    pub acc: [u64; 2],
    /// Frames advanced with `OutputMode::Present`. Not part of the checksum:
    /// it is an observation of the *session*, not of the simulation, and the
    /// two peers legitimately disagree about it after a rollback.
    pub presented: u64,
}

impl Simulation for CounterSim {
    fn save_state(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(STATE_BYTES);
        out.extend_from_slice(&self.frame.to_le_bytes());
        out.extend_from_slice(&self.acc[0].to_le_bytes());
        out.extend_from_slice(&self.acc[1].to_le_bytes());
        out
    }

    fn load_state(&mut self, data: &[u8]) -> Result<(), SimulationError> {
        if data.len() != STATE_BYTES {
            return Err(SimulationError::StateSize {
                expected: STATE_BYTES,
                actual: data.len(),
            });
        }
        let word = |i: usize| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&data[i * 8..i * 8 + 8]);
            u64::from_le_bytes(b)
        };
        self.frame = word(0);
        self.acc = [word(1), word(2)];
        Ok(())
    }

    fn advance_frame(&mut self, inputs: [PlayerInput; 2], output_mode: OutputMode) {
        for (i, input) in inputs.iter().enumerate() {
            self.acc[i] = self.acc[i]
                .wrapping_mul(0x100_0000_01B3)
                .wrapping_add(u64::from(input.bits()))
                .rotate_left((self.frame % 61) as u32 + 1);
        }
        self.frame += 1;
        if output_mode.emits_output() {
            self.presented += 1;
        }
    }

    fn checksum(&self) -> u64 {
        self.frame.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ self.acc[0].rotate_left(17)
            ^ self.acc[1].rotate_left(43)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips() {
        let mut a = CounterSim::default();
        for f in 0..50u16 {
            a.advance_frame([PlayerInput(f), PlayerInput(f * 3)], OutputMode::Present);
        }
        let blob = a.save_state();
        let mut b = CounterSim::default();
        b.load_state(&blob).unwrap();
        assert_eq!(a.checksum(), b.checksum());
    }

    #[test]
    fn a_short_state_blob_is_rejected() {
        let mut s = CounterSim::default();
        assert!(matches!(
            s.load_state(&[0u8; 4]),
            Err(SimulationError::StateSize {
                expected: 24,
                actual: 4
            })
        ));
    }

    #[test]
    fn input_order_changes_the_checksum() {
        let mut a = CounterSim::default();
        let mut b = CounterSim::default();
        a.advance_frame([PlayerInput(1), PlayerInput(0)], OutputMode::Present);
        a.advance_frame([PlayerInput(2), PlayerInput(0)], OutputMode::Present);
        b.advance_frame([PlayerInput(2), PlayerInput(0)], OutputMode::Present);
        b.advance_frame([PlayerInput(1), PlayerInput(0)], OutputMode::Present);
        assert_ne!(a.checksum(), b.checksum());
    }

    #[test]
    fn output_mode_does_not_touch_simulation_state() {
        let mut present = CounterSim::default();
        let mut resim = CounterSim::default();
        for f in 0..20u16 {
            present.advance_frame([PlayerInput(f), PlayerInput(f)], OutputMode::Present);
            resim.advance_frame([PlayerInput(f), PlayerInput(f)], OutputMode::Resimulate);
        }
        assert_eq!(present.checksum(), resim.checksum());
        assert_eq!(present.save_state(), resim.save_state());
        assert_ne!(present.presented, resim.presented);
    }
}
