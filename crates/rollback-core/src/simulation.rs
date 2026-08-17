//! The contract every rollback-capable simulation must satisfy.

use crate::input::PlayerInput;

/// Why a frame is being advanced.
///
/// `Resimulate` frames are replays of frames the session already showed the
/// player once. Anything user-visible or externally observable -- video, audio,
/// rumble, log lines -- must be suppressed for them, otherwise a single
/// rollback would replay several frames of sound in one display frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutputMode {
    /// The frame the player is about to see. Emit video and audio.
    Present,
    /// A frame being replayed to catch up after a rollback. Discard output.
    Resimulate,
}

impl OutputMode {
    /// True when the caller should emit video/audio for this frame.
    pub const fn emits_output(self) -> bool {
        matches!(self, OutputMode::Present)
    }
}

/// A deterministic, snapshot-able two-player simulation.
///
/// The whole rollback scheme rests on one invariant: for a given saved state
/// and a given input sequence, `advance_frame` must produce bit-identical
/// results on every machine, every run, in debug and in release. `OutputMode`
/// is explicitly *not* allowed to influence simulation state -- only output.
pub trait Simulation {
    /// Serialise the complete simulation state.
    fn save_state(&self) -> Vec<u8>;

    /// Restore a state previously produced by [`Simulation::save_state`].
    fn load_state(&mut self, data: &[u8]) -> Result<(), SimulationError>;

    /// Advance exactly one frame. `inputs` is indexed by [`crate::PlayerHandle::index`].
    fn advance_frame(&mut self, inputs: [PlayerInput; 2], output_mode: OutputMode);

    /// A cheap hash of the full simulation state, used for desync detection.
    ///
    /// Must depend on everything `save_state` captures and nothing else.
    fn checksum(&self) -> u64;
}

#[derive(Debug, thiserror::Error)]
pub enum SimulationError {
    #[error("state blob has wrong length: expected {expected}, got {actual}")]
    StateSize { expected: usize, actual: usize },
    #[error("state blob is malformed: {0}")]
    Malformed(String),
    #[error("simulation backend failed: {0}")]
    Backend(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_present_emits_output() {
        assert!(OutputMode::Present.emits_output());
        assert!(!OutputMode::Resimulate.emits_output());
    }
}
