//! Telemetry: a Prometheus exporter, a JSONL session log and the snapshot
//! struct they share.
//!
//! Three consumers, one source of truth:
//!
//! * Prometheus scrapes [`exporter`] at `127.0.0.1:9898/metrics` for live
//!   dashboards;
//! * [`jsonl`] writes the same numbers to disk for after-the-fact analysis and
//!   for `just collect`;
//! * `rollback-report` reads those files back to build `summary.csv` and the
//!   HTML report.

#![forbid(unsafe_code)]

pub mod exporter;
pub mod jsonl;
pub mod snapshot;

pub use exporter::{render, Exporter, DEFAULT_EXPORTER_ADDR};
pub use jsonl::{Record, SessionLog};
pub use snapshot::{MetricsSnapshot, Peer, ProcessStats, SessionInfo};
