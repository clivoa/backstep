//! Reading a JSONL session log back.
//!
//! Parsed field by field out of `serde_json::Value` rather than through the
//! producer's own types. That is deliberate: the report has to read logs from
//! *older* runs, including ones written before a field existed, and a strongly
//! typed `Deserialize` would reject the whole file over one missing key. A
//! missing number reads as zero and the report says so.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

/// One session's headline numbers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionSummary {
    pub name: String,
    pub path: PathBuf,
    pub simulation: String,
    pub profile: String,
    pub player: String,
    pub mode: String,
    pub commit: String,
    pub seed: u64,
    pub input_delay: u64,
    pub prediction_limit: u64,

    pub duration_s: f64,
    pub frames_presented: u64,
    pub frames_resimulated: u64,
    pub rollbacks: u64,
    pub max_rollback_depth: u64,
    pub predicted_frames: u64,
    pub mispredicted_frames: u64,
    pub stalls: u64,
    pub checksums_compared: u64,
    pub state_bytes: u64,
    pub desync: bool,
    /// True when the log ends with a `session_end` record.
    pub complete: bool,

    pub srtt_ms: f64,
    pub rttvar_ms: f64,
    pub loss_ratio: f64,
    pub duplicates: u64,
    pub reordered: u64,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,

    pub cpu_seconds: f64,
    pub resident_bytes: u64,

    /// What the peer reported about itself, if it ever did.
    pub remote_rollbacks: Option<u64>,
    pub remote_frames_presented: Option<u64>,
}

impl SessionSummary {
    pub fn prediction_accuracy(&self) -> f64 {
        if self.predicted_frames == 0 {
            return 1.0;
        }
        self.predicted_frames.saturating_sub(self.mispredicted_frames) as f64
            / self.predicted_frames as f64
    }

    pub fn mean_rollback_depth(&self) -> f64 {
        if self.rollbacks == 0 {
            return 0.0;
        }
        self.frames_resimulated as f64 / self.rollbacks as f64
    }

    /// Extra simulation work as a multiple of presented frames.
    pub fn resimulation_overhead(&self) -> f64 {
        if self.frames_presented == 0 {
            return 0.0;
        }
        self.frames_resimulated as f64 / self.frames_presented as f64
    }

    pub fn send_bitrate(&self) -> f64 {
        if self.duration_s <= 0.0 {
            return 0.0;
        }
        self.bytes_sent as f64 * 8.0 / self.duration_s
    }

    pub fn effective_fps(&self) -> f64 {
        if self.duration_s <= 0.0 {
            return 0.0;
        }
        self.frames_presented as f64 / self.duration_s
    }

    /// The key sessions are grouped by in the report.
    pub fn group(&self) -> (String, String, String) {
        (
            self.simulation.clone(),
            self.mode.clone(),
            self.profile.clone(),
        )
    }
}

/// One periodic sample, for the charts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimePoint {
    pub t_s: f64,
    pub rollbacks: u64,
    pub srtt_ms: f64,
    pub prediction_depth: u64,
    pub loss_ratio: f64,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub summary: SessionSummary,
    pub series: Vec<TimePoint>,
}

fn u(v: &Value, path: &[&str]) -> u64 {
    dig(v, path).and_then(|x| x.as_u64()).unwrap_or(0)
}

fn f(v: &Value, path: &[&str]) -> f64 {
    dig(v, path).and_then(|x| x.as_f64()).unwrap_or(0.0)
}

fn s(v: &Value, path: &[&str]) -> String {
    dig(v, path)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
}

fn dig<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = v;
    for key in path {
        current = current.get(key)?;
    }
    Some(current)
}

/// Inbound loss inferred from what the peer's sequence numbers implied.
fn loss_ratio(link: &Value) -> f64 {
    let highest = dig(link, &["highest_sequence"])
        .and_then(|x| x.as_i64())
        .unwrap_or(-1);
    let expected = (highest + 1).max(0) as f64;
    if expected <= 0.0 {
        return 0.0;
    }
    let unique = u(link, &["unique_received"]) as f64;
    ((expected - unique).max(0.0)) / expected
}

/// Fold a snapshot object into the summary.
fn apply_snapshot(summary: &mut SessionSummary, snapshot: &Value) {
    summary.duration_s = u(snapshot, &["elapsed_ms"]) as f64 / 1000.0;
    summary.frames_presented = u(snapshot, &["local", "frames_presented"]);
    summary.frames_resimulated = u(snapshot, &["local", "frames_resimulated"]);
    summary.rollbacks = u(snapshot, &["local", "rollbacks"]);
    summary.max_rollback_depth = u(snapshot, &["local", "max_rollback_depth"]);
    summary.predicted_frames = u(snapshot, &["local", "predicted_frames"]);
    summary.mispredicted_frames = u(snapshot, &["local", "mispredicted_frames"]);
    summary.stalls = u(snapshot, &["local", "stalls"]);
    summary.checksums_compared = u(snapshot, &["local", "checksums_compared"]);
    summary.state_bytes = u(snapshot, &["local", "state_bytes_last"]);
    summary.desync = dig(snapshot, &["desync"]).and_then(|x| x.as_bool()).unwrap_or(false);

    summary.srtt_ms = u(snapshot, &["link", "srtt_micros"]) as f64 / 1000.0;
    summary.rttvar_ms = u(snapshot, &["link", "rttvar_micros"]) as f64 / 1000.0;
    summary.duplicates = u(snapshot, &["link", "duplicates_received"]);
    summary.reordered = u(snapshot, &["link", "reordered_received"]);
    summary.packets_sent = u(snapshot, &["link", "packets_sent"]);
    summary.packets_received = u(snapshot, &["link", "packets_received"]);
    summary.bytes_sent = u(snapshot, &["link", "bytes_sent"]);
    summary.bytes_received = u(snapshot, &["link", "bytes_received"]);
    if let Some(link) = dig(snapshot, &["link"]) {
        summary.loss_ratio = loss_ratio(link);
    }

    summary.cpu_seconds = f(snapshot, &["process", "cpu_seconds"]);
    summary.resident_bytes = u(snapshot, &["process", "resident_bytes"]);

    if dig(snapshot, &["remote"]).is_some_and(|r| !r.is_null()) {
        summary.remote_rollbacks = Some(u(snapshot, &["remote", "rollbacks"]));
        summary.remote_frames_presented = Some(u(snapshot, &["remote", "frames_presented"]));
    }
}

/// Read one JSONL log.
///
/// A truncated file -- a session that crashed, or one still being written -- is
/// not an error: the summary is built from whatever records did land and marked
/// `complete: false`.
pub fn read_session(path: &Path) -> Result<Session> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;

    let mut summary = SessionSummary {
        name: path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        path: path.to_path_buf(),
        ..Default::default()
    };
    // The mode is the last dash-separated component of the session name.
    summary.mode = summary
        .name
        .rsplit('-')
        .next()
        .unwrap_or("unknown")
        .to_string();

    let mut series = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // A half-written final line is expected after a crash. Stop there.
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            break;
        };

        match value.get("record").and_then(|r| r.as_str()) {
            Some("session_start") => {
                summary.simulation = s(&value, &["info", "simulation"]);
                summary.profile = s(&value, &["info", "profile"]);
                summary.player = s(&value, &["info", "player"]);
                summary.commit = s(&value, &["info", "app_commit"]);
                summary.seed = u(&value, &["info", "seed"]);
                summary.input_delay = u(&value, &["info", "input_delay"]);
                summary.prediction_limit = u(&value, &["info", "prediction_limit"]);
            }
            Some("metrics") => {
                apply_snapshot(&mut summary, &value);
                series.push(TimePoint {
                    t_s: u(&value, &["t_ms"]) as f64 / 1000.0,
                    rollbacks: summary.rollbacks,
                    srtt_ms: summary.srtt_ms,
                    prediction_depth: u(&value, &["prediction_depth"]),
                    loss_ratio: summary.loss_ratio,
                });
            }
            Some("session_end") => {
                apply_snapshot(&mut summary, &value);
                summary.complete = true;
            }
            _ => {}
        }
    }

    Ok(Session { summary, series })
}

/// Read every `*.jsonl` in `dir`, sorted by name so the report is stable.
pub fn read_dir(dir: &Path) -> Result<Vec<Session>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    paths.sort();

    paths.iter().map(|p| read_session(p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_log(tag: &str, lines: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rollback-report-{}-{tag}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("1700000000-arena-combined-p1-bench.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        path
    }

    const START: &str = r#"{"record":"session_start","started_unix_ms":1700000000000,"info":{"simulation":"arena","profile":"combined","player":"p1","app_commit":"abc123","core_sha256":"","rom_sha256":"","seed":42,"input_delay":1,"prediction_limit":8,"state_history":16}}"#;

    fn snapshot_line(record: &str, t_ms: u64, rollbacks: u64) -> String {
        format!(
            r#"{{"record":"{record}","t_ms":{t_ms},"reason":"normal","elapsed_ms":{t_ms},"frame":10800,"confirmed_frame":10790,"prediction_depth":3,"local":{{"frames_presented":10800,"frames_resimulated":{},"rollbacks":{rollbacks},"max_rollback_depth":6,"predicted_frames":4000,"mispredicted_frames":1000,"stalls":2,"checksums_compared":178,"state_bytes_last":204,"advance_nanos":1,"save_state_nanos":2,"load_state_nanos":3,"last_rollback_depth":4}},"link":{{"packets_sent":10800,"bytes_sent":972000,"packets_received":10700,"bytes_received":963000,"unique_received":10600,"duplicates_received":12,"reordered_received":30,"auth_failures":0,"malformed":0,"highest_sequence":10799,"srtt_micros":41000,"rttvar_micros":7000,"rtt_samples":9000}},"remote":{{"frames_presented":10799,"rollbacks":399,"frames_resimulated":1,"max_rollback_depth":5,"predicted_frames":1,"mispredicted_frames":1,"stalls":0,"checksums_compared":178,"state_bytes_last":204,"srtt_micros":41000,"rttvar_micros":7000,"packets_sent":1,"packets_received":1,"bytes_sent":1,"bytes_received":1,"inferred_lost":1,"duplicates":1,"reordered":1}},"process":{{"cpu_seconds":12.5,"resident_bytes":52428800}},"desync":false,"info":{{"simulation":"arena","profile":"combined","player":"p1","app_commit":"abc123","core_sha256":"","rom_sha256":"","seed":42,"input_delay":1,"prediction_limit":8,"state_history":16}}}}"#,
            rollbacks * 4
        )
    }

    #[test]
    fn a_complete_log_yields_a_full_summary() {
        let path = write_log("complete", &[START, &snapshot_line("session_end", 180_000, 421)]);
        let session = read_session(&path).unwrap();
        let s = &session.summary;

        assert!(s.complete);
        assert_eq!(s.simulation, "arena");
        assert_eq!(s.profile, "combined");
        assert_eq!(s.player, "p1");
        assert_eq!(s.mode, "bench");
        assert_eq!(s.seed, 42);
        assert_eq!(s.duration_s, 180.0);
        assert_eq!(s.rollbacks, 421);
        assert_eq!(s.frames_presented, 10_800);
        assert_eq!(s.max_rollback_depth, 6);
        assert!((s.srtt_ms - 41.0).abs() < 1e-9);
        assert!((s.prediction_accuracy() - 0.75).abs() < 1e-9);
        assert!((s.mean_rollback_depth() - 4.0).abs() < 1e-9);
        assert_eq!(s.remote_rollbacks, Some(399));
        assert!(!s.desync);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn loss_is_inferred_from_the_sequence_run() {
        let path = write_log("loss", &[START, &snapshot_line("session_end", 180_000, 1)]);
        let s = read_session(&path).unwrap().summary;
        // 10800 expected, 10600 unique.
        assert!((s.loss_ratio - 200.0 / 10800.0).abs() < 1e-9);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn periodic_snapshots_become_the_time_series() {
        let path = write_log(
            "series",
            &[
                START,
                &snapshot_line("metrics", 1_000, 3),
                &snapshot_line("metrics", 2_000, 9),
                &snapshot_line("session_end", 3_000, 12),
            ],
        );
        let session = read_session(&path).unwrap();
        assert_eq!(session.series.len(), 2);
        assert_eq!(session.series[0].t_s, 1.0);
        assert_eq!(session.series[1].rollbacks, 9);
        assert_eq!(session.summary.rollbacks, 12, "the end record wins");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_truncated_log_is_summarised_and_flagged_incomplete() {
        let path = write_log(
            "truncated",
            &[START, &snapshot_line("metrics", 5_000, 7), "{\"record\":\"met"],
        );
        let session = read_session(&path).unwrap();
        assert!(!session.summary.complete);
        assert_eq!(session.summary.rollbacks, 7, "what did land is still used");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_log_with_only_a_header_does_not_divide_by_zero() {
        let path = write_log("header", &[START]);
        let s = read_session(&path).unwrap().summary;
        assert_eq!(s.prediction_accuracy(), 1.0);
        assert_eq!(s.mean_rollback_depth(), 0.0);
        assert_eq!(s.resimulation_overhead(), 0.0);
        assert_eq!(s.send_bitrate(), 0.0);
        assert_eq!(s.effective_fps(), 0.0);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn reading_a_directory_finds_every_log_in_order() {
        let path = write_log("dir", &[START, &snapshot_line("session_end", 1_000, 1)]);
        let dir = path.parent().unwrap();
        std::fs::copy(&path, dir.join("0000000000-arena-natural-p2-bench.jsonl")).unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();

        let sessions = read_dir(dir).unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions[0].summary.name.starts_with("0000000000"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn sessions_group_by_simulation_mode_and_profile() {
        let path = write_log("group", &[START, &snapshot_line("session_end", 1_000, 1)]);
        let s = read_session(&path).unwrap().summary;
        assert_eq!(
            s.group(),
            ("arena".into(), "bench".into(), "combined".into())
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
