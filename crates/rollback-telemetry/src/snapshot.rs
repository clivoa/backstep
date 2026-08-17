//! The numbers the exporter, the JSONL log and the report all read from.
//!
//! A snapshot is a plain `Copy`-ish struct assembled once per frame from the
//! session, the transport and `/proc`. Keeping it separate from the sources
//! means the exporter never has to reach into a live session -- it renders a
//! value that was consistent at some instant, rather than a mix of fields read
//! at different points of a frame.

use rollback_core::{SessionStats, SimulationKind};
use rollback_net::{LinkStats, TelemetrySummary};
use serde::{Deserialize, Serialize};

/// Which end of the link a set of numbers describes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Peer {
    Local,
    Remote,
}

impl Peer {
    pub const fn as_str(self) -> &'static str {
        match self {
            Peer::Local => "local",
            Peer::Remote => "remote",
        }
    }
}

/// Static facts about the session, exported once as an info metric.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub simulation: String,
    pub profile: String,
    pub player: String,
    pub app_commit: String,
    pub core_sha256: String,
    pub rom_sha256: String,
    pub seed: u64,
    pub input_delay: u8,
    pub prediction_limit: u8,
    pub state_history: u8,
}

impl SessionInfo {
    pub fn new(simulation: SimulationKind, profile: &str, player: &str) -> SessionInfo {
        SessionInfo {
            simulation: simulation.as_str().to_string(),
            profile: profile.to_string(),
            player: player.to_string(),
            app_commit: String::new(),
            core_sha256: String::new(),
            rom_sha256: String::new(),
            seed: 0,
            input_delay: 0,
            prediction_limit: 0,
            state_history: 0,
        }
    }
}

/// Process CPU and memory, sampled from `/proc`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProcessStats {
    pub cpu_seconds: f64,
    pub resident_bytes: u64,
}

impl ProcessStats {
    /// Read `/proc/self/stat` and `/proc/self/statm`.
    ///
    /// Returns the default (all zeroes) on any platform or permission problem:
    /// missing resource numbers must not take a session down.
    pub fn sample() -> ProcessStats {
        let mut out = ProcessStats::default();
        let ticks_per_second = 100.0; // _SC_CLK_TCK is 100 on every Linux we target.

        if let Ok(stat) = std::fs::read_to_string("/proc/self/stat") {
            // utime and stime are fields 14 and 15 counting from 1. They cannot
            // be found by splitting the whole line, because field 2 (`comm`) is
            // the executable name in parentheses and may itself contain spaces
            // -- so split after the last ')' and count from there.
            if let Some((_, after_comm)) = stat.rsplit_once(')') {
                let fields: Vec<&str> = after_comm.split_whitespace().collect();
                let utime = fields.get(11).and_then(|v| v.parse::<f64>().ok());
                let stime = fields.get(12).and_then(|v| v.parse::<f64>().ok());
                if let (Some(u), Some(s)) = (utime, stime) {
                    out.cpu_seconds = (u + s) / ticks_per_second;
                }
            }
        }
        if let Ok(statm) = std::fs::read_to_string("/proc/self/statm") {
            // Second field is the resident set size, in pages.
            if let Some(resident_pages) = statm.split_whitespace().nth(1) {
                if let Ok(pages) = resident_pages.parse::<u64>() {
                    out.resident_bytes = pages * PAGE_BYTES;
                }
            }
        }
        out
    }
}

/// Page size on x86_64 Linux, which is the only target this lab builds for.
const PAGE_BYTES: u64 = 4096;

/// Everything worth exporting at one instant.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub info: SessionInfo,
    pub elapsed_ms: u64,
    pub frame: i32,
    pub confirmed_frame: i32,
    pub prediction_depth: u32,
    pub local: SessionStats,
    pub link: LinkStats,
    /// The peer's own summary, as it last reported it.
    pub remote: Option<TelemetrySummary>,
    pub process: ProcessStats,
    pub desync: bool,
}

impl MetricsSnapshot {
    pub fn new(info: SessionInfo) -> MetricsSnapshot {
        MetricsSnapshot {
            info,
            elapsed_ms: 0,
            frame: 0,
            confirmed_frame: -1,
            prediction_depth: 0,
            local: SessionStats::default(),
            link: LinkStats::default(),
            remote: None,
            process: ProcessStats::default(),
            desync: false,
        }
    }

    /// Build the summary this peer sends to the other one.
    pub fn to_summary(&self) -> TelemetrySummary {
        TelemetrySummary {
            frames_presented: self.local.frames_presented,
            frames_resimulated: self.local.frames_resimulated,
            rollbacks: self.local.rollbacks,
            max_rollback_depth: self.local.max_rollback_depth,
            predicted_frames: self.local.predicted_frames,
            mispredicted_frames: self.local.mispredicted_frames,
            stalls: self.local.stalls,
            checksums_compared: self.local.checksums_compared,
            state_bytes_last: self.local.state_bytes_last as u32,
            srtt_micros: self.link.srtt_micros,
            rttvar_micros: self.link.rttvar_micros,
            packets_sent: self.link.packets_sent,
            packets_received: self.link.packets_received,
            bytes_sent: self.link.bytes_sent,
            bytes_received: self.link.bytes_received,
            inferred_lost: self.link.inferred_lost(),
            duplicates: self.link.duplicates_received,
            reordered: self.link.reordered_received,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_stats_read_something_plausible_on_linux() {
        let stats = ProcessStats::sample();
        if cfg!(target_os = "linux") {
            assert!(
                stats.resident_bytes > 1024 * 1024,
                "a running test process holds at least a megabyte, got {}",
                stats.resident_bytes
            );
            assert!(stats.cpu_seconds >= 0.0);
        }
    }

    #[test]
    fn the_summary_mirrors_the_local_counters() {
        let mut snap = MetricsSnapshot::new(SessionInfo::new(SimulationKind::Arena, "natural", "p1"));
        snap.local.frames_presented = 10_800;
        snap.local.rollbacks = 421;
        snap.local.max_rollback_depth = 8;
        snap.link.srtt_micros = 32_000;

        let summary = snap.to_summary();
        assert_eq!(summary.frames_presented, 10_800);
        assert_eq!(summary.rollbacks, 421);
        assert_eq!(summary.max_rollback_depth, 8);
        assert_eq!(summary.srtt_micros, 32_000);
    }

    #[test]
    fn peer_labels_are_stable() {
        assert_eq!(Peer::Local.as_str(), "local");
        assert_eq!(Peer::Remote.as_str(), "remote");
    }
}
