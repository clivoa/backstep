//! `summary.csv`: one row per session, every derived figure included.
//!
//! Written by hand rather than with a CSV crate because the only escaping the
//! data needs is for the commit and profile strings, and a tool this small
//! should not need a dependency to emit thirty commas.

use crate::parse::SessionSummary;

/// Column headers, in order. Exposed so the tests can assert every row matches.
pub const COLUMNS: &[&str] = &[
    "session",
    "simulation",
    "mode",
    "profile",
    "player",
    "commit",
    "seed",
    "input_delay",
    "prediction_limit",
    "complete",
    "duration_s",
    "frames_presented",
    "effective_fps",
    "frames_resimulated",
    "resimulation_overhead",
    "rollbacks",
    "mean_rollback_depth",
    "max_rollback_depth",
    "predicted_frames",
    "mispredicted_frames",
    "prediction_accuracy",
    "stalls",
    "checksums_compared",
    "desync",
    "state_bytes",
    "srtt_ms",
    "rttvar_ms",
    "loss_pct",
    "duplicates",
    "reordered",
    "packets_sent",
    "packets_received",
    "send_bitrate_bps",
    "cpu_seconds",
    "resident_mb",
    "remote_rollbacks",
    "remote_frames_presented",
];

pub fn render(sessions: &[SessionSummary]) -> String {
    let mut out = String::with_capacity(1024 + sessions.len() * 256);
    out.push_str(&COLUMNS.join(","));
    out.push('\n');
    for s in sessions {
        out.push_str(&row(s));
        out.push('\n');
    }
    out
}

fn row(s: &SessionSummary) -> String {
    let fields: Vec<String> = vec![
        quote(&s.name),
        quote(&s.simulation),
        quote(&s.mode),
        quote(&s.profile),
        quote(&s.player),
        quote(&s.commit),
        s.seed.to_string(),
        s.input_delay.to_string(),
        s.prediction_limit.to_string(),
        s.complete.to_string(),
        format!("{:.3}", s.duration_s),
        s.frames_presented.to_string(),
        format!("{:.2}", s.effective_fps()),
        s.frames_resimulated.to_string(),
        format!("{:.4}", s.resimulation_overhead()),
        s.rollbacks.to_string(),
        format!("{:.2}", s.mean_rollback_depth()),
        s.max_rollback_depth.to_string(),
        s.predicted_frames.to_string(),
        s.mispredicted_frames.to_string(),
        format!("{:.4}", s.prediction_accuracy()),
        s.stalls.to_string(),
        s.checksums_compared.to_string(),
        s.desync.to_string(),
        s.state_bytes.to_string(),
        format!("{:.2}", s.srtt_ms),
        format!("{:.2}", s.rttvar_ms),
        format!("{:.3}", s.loss_ratio * 100.0),
        s.duplicates.to_string(),
        s.reordered.to_string(),
        s.packets_sent.to_string(),
        s.packets_received.to_string(),
        format!("{:.0}", s.send_bitrate()),
        format!("{:.2}", s.cpu_seconds),
        format!("{:.1}", s.resident_bytes as f64 / (1024.0 * 1024.0)),
        s.remote_rollbacks.map_or(String::new(), |v| v.to_string()),
        s.remote_frames_presented
            .map_or(String::new(), |v| v.to_string()),
    ];
    debug_assert_eq!(fields.len(), COLUMNS.len());
    fields.join(",")
}

/// RFC 4180 quoting, applied only when the value needs it.
fn quote(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SessionSummary {
        SessionSummary {
            name: "1700000000-arena-combined-p1-bench".into(),
            simulation: "arena".into(),
            mode: "bench".into(),
            profile: "combined".into(),
            player: "p1".into(),
            commit: "abc123".into(),
            seed: 42,
            input_delay: 1,
            prediction_limit: 8,
            complete: true,
            duration_s: 180.0,
            frames_presented: 10_800,
            frames_resimulated: 1_684,
            rollbacks: 421,
            max_rollback_depth: 6,
            predicted_frames: 4_000,
            mispredicted_frames: 1_000,
            stalls: 2,
            checksums_compared: 178,
            state_bytes: 204,
            srtt_ms: 41.0,
            rttvar_ms: 7.0,
            loss_ratio: 0.0185,
            duplicates: 12,
            reordered: 30,
            packets_sent: 10_800,
            packets_received: 10_700,
            bytes_sent: 972_000,
            bytes_received: 963_000,
            cpu_seconds: 12.5,
            resident_bytes: 52_428_800,
            remote_rollbacks: Some(399),
            ..Default::default()
        }
    }

    fn parse_rows(csv: &str) -> Vec<Vec<String>> {
        csv.lines()
            .map(|l| l.split(',').map(|s| s.to_string()).collect())
            .collect()
    }

    #[test]
    fn the_header_matches_every_row() {
        let csv = render(&[sample(), sample()]);
        let rows = parse_rows(&csv);
        assert_eq!(rows[0].len(), COLUMNS.len());
        for row in &rows[1..] {
            assert_eq!(row.len(), COLUMNS.len(), "row width differs from the header");
        }
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn derived_columns_are_computed_not_copied() {
        let csv = render(&[sample()]);
        let header = parse_rows(&csv)[0].clone();
        let row = parse_rows(&csv)[1].clone();
        let get = |name: &str| {
            let i = header.iter().position(|h| h == name).unwrap();
            row[i].clone()
        };
        assert_eq!(get("prediction_accuracy"), "0.7500");
        assert_eq!(get("mean_rollback_depth"), "4.00");
        assert_eq!(get("effective_fps"), "60.00");
        assert_eq!(get("loss_pct"), "1.850");
        assert_eq!(get("resident_mb"), "50.0");
    }

    #[test]
    fn an_empty_run_still_produces_a_header() {
        let csv = render(&[]);
        assert_eq!(csv.trim(), COLUMNS.join(","));
    }

    #[test]
    fn a_missing_remote_summary_leaves_the_column_empty() {
        let mut s = sample();
        s.remote_rollbacks = None;
        s.remote_frames_presented = None;
        let csv = render(&[s]);
        assert!(csv.trim_end().ends_with(",,"), "got {csv:?}");
    }

    #[test]
    fn values_with_commas_and_quotes_are_escaped() {
        let mut s = sample();
        s.profile = "delay,20".into();
        s.commit = "a\"b".into();
        let csv = render(&[s]);
        assert!(csv.contains("\"delay,20\""));
        assert!(csv.contains("\"a\"\"b\""));
    }

    #[test]
    fn quoting_leaves_ordinary_values_alone() {
        assert_eq!(quote("arena"), "arena");
        assert_eq!(quote("p1"), "p1");
    }
}
