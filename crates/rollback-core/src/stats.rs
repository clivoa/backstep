//! Counters the session keeps so telemetry and reports have something to read.
//!
//! These are pure observations: nothing here feeds back into the simulation,
//! so a session with statistics disabled would produce identical frames.

use crate::config::Frame;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStats {
    /// Frames advanced in `Present` mode -- i.e. frames the player saw.
    pub frames_presented: u64,
    /// Frames advanced in `Resimulate` mode, summed over every rollback.
    pub frames_resimulated: u64,
    /// Number of rollbacks performed.
    pub rollbacks: u64,
    /// Deepest single rollback, in frames.
    pub max_rollback_depth: u32,
    /// Depth of the most recent rollback, in frames.
    pub last_rollback_depth: u32,
    /// Frames first simulated with a predicted remote input.
    pub predicted_frames: u64,
    /// Predicted frames that later turned out to be wrong.
    pub mispredicted_frames: u64,
    /// Times the session refused to advance because the prediction window was full.
    pub stalls: u64,
    /// Confirmed-frame checksums compared against the peer.
    pub checksums_compared: u64,
    /// Size of the most recent saved state, in bytes.
    pub state_bytes_last: u64,
    /// Largest saved state observed, in bytes.
    pub state_bytes_max: u64,
    /// Cumulative time spent inside `advance_frame`, in nanoseconds.
    pub advance_nanos: u64,
    /// Cumulative time spent inside `save_state`, in nanoseconds.
    pub save_state_nanos: u64,
    /// Cumulative time spent inside `load_state`, in nanoseconds.
    pub load_state_nanos: u64,
}

impl SessionStats {
    /// Fraction of predicted frames that held up, in `[0, 1]`.
    ///
    /// Returns 1.0 when nothing was predicted: a session that never had to
    /// guess never guessed wrong.
    pub fn prediction_accuracy(&self) -> f64 {
        if self.predicted_frames == 0 {
            return 1.0;
        }
        let correct = self
            .predicted_frames
            .saturating_sub(self.mispredicted_frames);
        correct as f64 / self.predicted_frames as f64
    }

    /// Average number of frames replayed per rollback.
    pub fn mean_rollback_depth(&self) -> f64 {
        if self.rollbacks == 0 {
            return 0.0;
        }
        self.frames_resimulated as f64 / self.rollbacks as f64
    }

    /// Extra simulation work as a multiple of the presented frames.
    ///
    /// 0.0 means no rollback happened; 1.0 means the CPU simulated every frame
    /// twice on average.
    pub fn resimulation_overhead(&self) -> f64 {
        if self.frames_presented == 0 {
            return 0.0;
        }
        self.frames_resimulated as f64 / self.frames_presented as f64
    }

    pub(crate) fn record_rollback(&mut self, depth: u32) {
        self.rollbacks += 1;
        self.last_rollback_depth = depth;
        self.max_rollback_depth = self.max_rollback_depth.max(depth);
        self.frames_resimulated += u64::from(depth);
    }

    pub(crate) fn record_state_size(&mut self, bytes: usize) {
        let bytes = bytes as u64;
        self.state_bytes_last = bytes;
        self.state_bytes_max = self.state_bytes_max.max(bytes);
    }
}

/// Something worth writing to the JSONL session log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SessionEvent {
    /// A frame was presented to the player.
    Advanced { frame: Frame, predicted: bool },
    /// The session refused to advance: too far ahead of the peer.
    Stalled { frame: Frame, waiting_for: Frame },
    /// A misprediction was corrected.
    RolledBack { from: Frame, to: Frame, depth: u32 },
    /// A confirmed-frame checksum matched the peer's.
    ChecksumMatched { frame: Frame, checksum: u64 },
    /// A confirmed-frame checksum did not match. The session is over.
    Desync {
        frame: Frame,
        local: u64,
        remote: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accuracy_of_a_session_that_never_predicted_is_one() {
        assert_eq!(SessionStats::default().prediction_accuracy(), 1.0);
    }

    #[test]
    fn accuracy_and_depth_use_the_recorded_counters() {
        let mut s = SessionStats {
            predicted_frames: 100,
            mispredicted_frames: 25,
            frames_presented: 200,
            ..Default::default()
        };
        assert_eq!(s.prediction_accuracy(), 0.75);

        s.record_rollback(4);
        s.record_rollback(6);
        assert_eq!(s.rollbacks, 2);
        assert_eq!(s.max_rollback_depth, 6);
        assert_eq!(s.last_rollback_depth, 6);
        assert_eq!(s.frames_resimulated, 10);
        assert_eq!(s.mean_rollback_depth(), 5.0);
        assert_eq!(s.resimulation_overhead(), 0.05);
    }

    #[test]
    fn state_size_tracks_last_and_max() {
        let mut s = SessionStats::default();
        s.record_state_size(512);
        s.record_state_size(128);
        assert_eq!(s.state_bytes_last, 128);
        assert_eq!(s.state_bytes_max, 512);
    }

    #[test]
    fn misprediction_cannot_push_accuracy_below_zero() {
        let s = SessionStats {
            predicted_frames: 5,
            mispredicted_frames: 9,
            ..Default::default()
        };
        assert_eq!(s.prediction_accuracy(), 0.0);
    }
}
