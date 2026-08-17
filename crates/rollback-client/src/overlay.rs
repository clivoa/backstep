//! The rollback overlay: what the netcode is doing, on screen, live.
//!
//! This is the reason the arena exists. Rollback is invisible when it works --
//! that is its whole purpose -- so the lab draws the machinery: which frames
//! were predicted, how often the prediction was wrong, how deep the corrections
//! went, and how far ahead of the peer we currently are.

use rollback_telemetry::MetricsSnapshot;

/// Frames of history in the strip at the bottom of the overlay.
pub const HISTORY_LEN: usize = 180;

/// How one frame was produced.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FrameMark {
    /// Both inputs were known: nothing was guessed.
    #[default]
    Confirmed,
    /// The remote input was predicted.
    Predicted,
    /// A rollback corrected this frame.
    Corrected,
    /// The session refused to advance.
    Stalled,
}

impl FrameMark {
    /// RGB for the history strip.
    pub const fn colour(self) -> (u8, u8, u8) {
        match self {
            FrameMark::Confirmed => (60, 140, 90),
            FrameMark::Predicted => (200, 170, 60),
            FrameMark::Corrected => (200, 80, 70),
            FrameMark::Stalled => (110, 110, 130),
        }
    }
}

/// A rolling window of recent frames plus the derived headline numbers.
pub struct Overlay {
    history: [FrameMark; HISTORY_LEN],
    /// Index of the next slot to write; the strip is a ring.
    cursor: usize,
    filled: usize,
    last_rollbacks: u64,
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

impl Overlay {
    pub fn new() -> Overlay {
        Overlay {
            history: [FrameMark::Confirmed; HISTORY_LEN],
            cursor: 0,
            filled: 0,
            last_rollbacks: 0,
        }
    }

    /// Record one frame.
    ///
    /// A rollback is attributed to the frame it was noticed on, not to the
    /// frames it actually replayed: those have already scrolled past, and
    /// rewriting history in the strip would hide how *late* the correction was,
    /// which is the interesting part.
    pub fn push(&mut self, mark: FrameMark, snapshot: &MetricsSnapshot) {
        let rolled_back = snapshot.local.rollbacks > self.last_rollbacks;
        self.last_rollbacks = snapshot.local.rollbacks;

        let mark = if rolled_back && mark != FrameMark::Stalled {
            FrameMark::Corrected
        } else {
            mark
        };

        self.history[self.cursor] = mark;
        self.cursor = (self.cursor + 1) % HISTORY_LEN;
        self.filled = (self.filled + 1).min(HISTORY_LEN);
    }

    /// The strip, oldest first.
    pub fn strip(&self) -> Vec<FrameMark> {
        if self.filled < HISTORY_LEN {
            return self.history[..self.filled].to_vec();
        }
        let mut out = Vec::with_capacity(HISTORY_LEN);
        out.extend_from_slice(&self.history[self.cursor..]);
        out.extend_from_slice(&self.history[..self.cursor]);
        out
    }

    /// The lines of text drawn in the corner.
    pub fn lines(&self, snapshot: &MetricsSnapshot) -> Vec<String> {
        let s = snapshot;
        vec![
            format!(
                "FRAME {} CONFIRMED {} AHEAD {}",
                s.frame, s.confirmed_frame, s.prediction_depth
            ),
            format!(
                "PREDICTED {} WRONG {} ACC {}%",
                s.local.predicted_frames,
                s.local.mispredicted_frames,
                (s.local.prediction_accuracy() * 100.0).round() as i64
            ),
            format!(
                "ROLLBACKS {} DEPTH {} MAX {}",
                s.local.rollbacks, s.local.last_rollback_depth, s.local.max_rollback_depth
            ),
            format!(
                "RESIM {} STALLS {} STATE {}B",
                s.local.frames_resimulated, s.local.stalls, s.local.state_bytes_last
            ),
            format!(
                "RTT {}MS VAR {}MS LOSS {}%",
                s.link.srtt_ms().round() as i64,
                s.link.rttvar_ms().round() as i64,
                (s.link.loss_ratio() * 100.0).round() as i64
            ),
            format!(
                "SENT {} RECV {} DUP {} REORD {}",
                s.link.packets_sent,
                s.link.packets_received,
                s.link.duplicates_received,
                s.link.reordered_received
            ),
            if s.desync {
                "DESYNC - SESSION OVER".to_string()
            } else {
                format!("PROFILE {}", s.info.profile.to_uppercase())
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rollback_core::SimulationKind;
    use rollback_telemetry::SessionInfo;

    fn snapshot() -> MetricsSnapshot {
        MetricsSnapshot::new(SessionInfo::new(SimulationKind::Arena, "combined", "p1"))
    }

    #[test]
    fn the_strip_grows_then_scrolls() {
        let mut overlay = Overlay::new();
        let snap = snapshot();
        for _ in 0..10 {
            overlay.push(FrameMark::Confirmed, &snap);
        }
        assert_eq!(overlay.strip().len(), 10);

        for _ in 0..HISTORY_LEN * 2 {
            overlay.push(FrameMark::Predicted, &snap);
        }
        let strip = overlay.strip();
        assert_eq!(strip.len(), HISTORY_LEN);
        assert!(strip.iter().all(|&m| m == FrameMark::Predicted));
    }

    #[test]
    fn the_strip_keeps_frames_in_order() {
        let mut overlay = Overlay::new();
        let snap = snapshot();
        for _ in 0..HISTORY_LEN {
            overlay.push(FrameMark::Confirmed, &snap);
        }
        overlay.push(FrameMark::Stalled, &snap);
        let strip = overlay.strip();
        assert_eq!(*strip.last().unwrap(), FrameMark::Stalled, "newest is last");
        assert_eq!(strip[0], FrameMark::Confirmed);
    }

    #[test]
    fn a_rollback_repaints_the_frame_it_was_noticed_on() {
        let mut overlay = Overlay::new();
        let mut snap = snapshot();
        overlay.push(FrameMark::Predicted, &snap);
        assert_eq!(overlay.strip()[0], FrameMark::Predicted);

        snap.local.rollbacks = 1;
        overlay.push(FrameMark::Predicted, &snap);
        assert_eq!(overlay.strip()[1], FrameMark::Corrected);

        // No new rollback: back to plain prediction.
        overlay.push(FrameMark::Predicted, &snap);
        assert_eq!(overlay.strip()[2], FrameMark::Predicted);
    }

    #[test]
    fn a_stall_is_never_overwritten_by_a_rollback() {
        let mut overlay = Overlay::new();
        let mut snap = snapshot();
        snap.local.rollbacks = 5;
        overlay.push(FrameMark::Stalled, &snap);
        assert_eq!(overlay.strip()[0], FrameMark::Stalled);
    }

    #[test]
    fn every_mark_has_a_distinct_colour() {
        let marks = [
            FrameMark::Confirmed,
            FrameMark::Predicted,
            FrameMark::Corrected,
            FrameMark::Stalled,
        ];
        let colours: std::collections::HashSet<(u8, u8, u8)> =
            marks.iter().map(|m| m.colour()).collect();
        assert_eq!(colours.len(), marks.len());
    }

    #[test]
    fn the_text_reports_the_headline_numbers() {
        let overlay = Overlay::new();
        let mut snap = snapshot();
        snap.frame = 1234;
        snap.local.rollbacks = 42;
        snap.local.predicted_frames = 100;
        snap.local.mispredicted_frames = 25;
        snap.link.srtt_micros = 33_000;

        let lines = overlay.lines(&snap);
        let text = lines.join("\n");
        assert!(text.contains("FRAME 1234"));
        assert!(text.contains("ROLLBACKS 42"));
        assert!(text.contains("ACC 75%"));
        assert!(text.contains("RTT 33MS"));
        assert!(text.contains("PROFILE COMBINED"));
    }

    #[test]
    fn a_desync_replaces_the_last_line() {
        let overlay = Overlay::new();
        let mut snap = snapshot();
        snap.desync = true;
        assert!(overlay.lines(&snap).last().unwrap().contains("DESYNC"));
    }

    #[test]
    fn every_line_is_drawable_by_the_bitmap_font() {
        let overlay = Overlay::new();
        let snap = snapshot();
        for line in overlay.lines(&snap) {
            for c in line.chars() {
                assert!(
                    crate::font::glyph(c).is_some(),
                    "overlay uses '{c}', which the font cannot draw"
                );
            }
        }
    }
}
