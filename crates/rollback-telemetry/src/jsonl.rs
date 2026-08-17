//! Per-session JSONL log.
//!
//! One JSON object per line, opened with a `session_start` record that carries
//! everything needed to interpret the rest. JSONL rather than a single JSON
//! document because a session that crashes mid-match still leaves a readable
//! file up to the last flushed line -- which is exactly when the log matters.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use rollback_core::{Frame, PlayerInput, SessionEvent};
use serde::{Deserialize, Serialize};

use crate::snapshot::MetricsSnapshot;

/// One line of the log.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum Record {
    /// Always the first line.
    SessionStart {
        started_unix_ms: u128,
        info: serde_json::Value,
    },
    /// A local input was queued for a frame.
    LocalInput { t_ms: u64, frame: Frame, input: u16 },
    /// A batch of remote inputs was accepted.
    RemoteInputs {
        t_ms: u64,
        start_frame: Frame,
        inputs: Vec<u16>,
    },
    /// A datagram was sent.
    Sent {
        t_ms: u64,
        sequence: u32,
        kind: &'static str,
        bytes: usize,
    },
    /// A datagram was received, authenticated and parsed.
    Received {
        t_ms: u64,
        sequence: u32,
        kind: &'static str,
        ack: Option<u32>,
    },
    /// Something the rollback session did.
    Session {
        t_ms: u64,
        #[serde(flatten)]
        event: SessionEvent,
    },
    /// A periodic snapshot of every counter.
    Metrics {
        t_ms: u64,
        #[serde(flatten)]
        snapshot: Box<MetricsSnapshot>,
    },
    /// Always the last line, if the session ends in an orderly way.
    SessionEnd {
        t_ms: u64,
        reason: String,
        #[serde(flatten)]
        snapshot: Box<MetricsSnapshot>,
    },
}

pub struct SessionLog {
    writer: BufWriter<File>,
    path: PathBuf,
    lines: u64,
}

impl SessionLog {
    /// Create `dir/<name>.jsonl` and write the opening record.
    pub fn create(
        dir: impl AsRef<Path>,
        name: &str,
        info: &impl Serialize,
    ) -> std::io::Result<SessionLog> {
        std::fs::create_dir_all(dir.as_ref())?;
        let path = dir.as_ref().join(format!("{name}.jsonl"));
        let file = File::create(&path)?;
        let mut log = SessionLog {
            writer: BufWriter::new(file),
            path,
            lines: 0,
        };
        let started_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        log.write(&Record::SessionStart {
            started_unix_ms,
            info: serde_json::to_value(info)?,
        })?;
        Ok(log)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn lines(&self) -> u64 {
        self.lines
    }

    pub fn write(&mut self, record: &Record) -> std::io::Result<()> {
        serde_json::to_writer(&mut self.writer, record)?;
        self.writer.write_all(b"\n")?;
        self.lines += 1;
        Ok(())
    }

    /// Convenience for the per-frame events the session hands back.
    pub fn write_events(&mut self, t_ms: u64, events: &[SessionEvent]) -> std::io::Result<()> {
        for &event in events {
            self.write(&Record::Session { t_ms, event })?;
        }
        Ok(())
    }

    pub fn write_local_input(
        &mut self,
        t_ms: u64,
        frame: Frame,
        input: PlayerInput,
    ) -> std::io::Result<()> {
        self.write(&Record::LocalInput {
            t_ms,
            frame,
            input: input.bits(),
        })
    }

    pub fn write_remote_inputs(
        &mut self,
        t_ms: u64,
        start_frame: Frame,
        inputs: &[PlayerInput],
    ) -> std::io::Result<()> {
        self.write(&Record::RemoteInputs {
            t_ms,
            start_frame,
            inputs: inputs.iter().map(|i| i.bits()).collect(),
        })
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }

    /// Write the closing record and flush.
    pub fn finish(mut self, t_ms: u64, reason: &str, snapshot: &MetricsSnapshot) -> std::io::Result<PathBuf> {
        self.write(&Record::SessionEnd {
            t_ms,
            reason: reason.to_string(),
            snapshot: Box::new(snapshot.clone()),
        })?;
        self.writer.flush()?;
        Ok(self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{MetricsSnapshot, SessionInfo};
    use rollback_core::SimulationKind;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rollback-jsonl-{}-{}-{:?}",
            std::process::id(),
            tag,
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn snapshot() -> MetricsSnapshot {
        MetricsSnapshot::new(SessionInfo::new(SimulationKind::Arena, "natural", "p1"))
    }

    #[test]
    fn every_line_is_standalone_json() {
        let dir = temp_dir("standalone");
        let mut log = SessionLog::create(&dir, "session", &snapshot().info).unwrap();
        log.write_local_input(16, 1, PlayerInput(0x0A)).unwrap();
        log.write_remote_inputs(20, 1, &[PlayerInput(1), PlayerInput(2)])
            .unwrap();
        log.write_events(
            33,
            &[
                SessionEvent::Advanced { frame: 1, predicted: true },
                SessionEvent::RolledBack { from: 5, to: 3, depth: 2 },
            ],
        )
        .unwrap();
        let path = log.finish(180_000, "normal", &snapshot()).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 6, "start + 4 + end");
        for line in &lines {
            let value: serde_json::Value = serde_json::from_str(line).expect("each line parses");
            assert!(value.get("record").is_some(), "each line is tagged");
        }
        assert!(lines[0].contains("session_start"));
        assert!(lines.last().unwrap().contains("session_end"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn session_events_keep_their_own_tag_when_flattened() {
        let dir = temp_dir("flatten");
        let mut log = SessionLog::create(&dir, "s", &snapshot().info).unwrap();
        log.write_events(1, &[SessionEvent::Desync { frame: 60, local: 1, remote: 2 }])
            .unwrap();
        let path = log.finish(2, "desync", &snapshot()).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let desync_line = text.lines().find(|l| l.contains("desync")).unwrap();
        let value: serde_json::Value = serde_json::from_str(desync_line).unwrap();
        assert_eq!(value["record"], "session");
        assert_eq!(value["event"], "desync");
        assert_eq!(value["frame"], 60);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_log_counts_what_it_wrote() {
        let dir = temp_dir("count");
        let mut log = SessionLog::create(&dir, "s", &snapshot().info).unwrap();
        assert_eq!(log.lines(), 1);
        for f in 0..10 {
            log.write_local_input(0, f, PlayerInput(0)).unwrap();
        }
        assert_eq!(log.lines(), 11);
        log.finish(0, "normal", &snapshot()).unwrap();
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_metrics_record_carries_the_whole_snapshot() {
        let dir = temp_dir("metrics");
        let mut snap = snapshot();
        snap.local.rollbacks = 7;
        let mut log = SessionLog::create(&dir, "s", &snap.info).unwrap();
        log.write(&Record::Metrics {
            t_ms: 1_000,
            snapshot: Box::new(snap.clone()),
        })
        .unwrap();
        let path = log.finish(2_000, "normal", &snap).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let line = text.lines().find(|l| l.contains("\"metrics\"")).unwrap();
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["local"]["rollbacks"], 7);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn creating_a_log_makes_the_directory() {
        let dir = temp_dir("mkdir").join("nested").join("deeper");
        let log = SessionLog::create(&dir, "s", &snapshot().info).unwrap();
        assert!(log.path().exists());
        std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap()).ok();
    }
}
