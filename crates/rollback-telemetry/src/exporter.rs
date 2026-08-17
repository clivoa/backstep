//! A Prometheus exporter on `127.0.0.1:9898/metrics`.
//!
//! Deliberately hand-rolled and deliberately loopback-only. The exposition
//! format is a handful of lines of text, and pulling in an HTTP stack for it
//! would add more attack surface than the whole UDP protocol has. Binding to
//! `127.0.0.1` is the reason the AWS security group can stay at "UDP/7000 only,
//! no dashboard exposed": there is nothing listening on a public interface.
//!
//! Prometheus scrapes the local exporter; the *remote* peer's numbers get here
//! over the session link as `TelemetrySummary`, not by scraping across the
//! Atlantic.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::snapshot::MetricsSnapshot;

/// Address the lab always exports on.
pub const DEFAULT_EXPORTER_ADDR: &str = "127.0.0.1:9898";

/// A handle to the background exporter thread.
pub struct Exporter {
    shared: Arc<Mutex<MetricsSnapshot>>,
    running: Arc<AtomicBool>,
    addr: SocketAddr,
}

impl Exporter {
    /// Start serving. The listener thread exits when [`Exporter::stop`] is
    /// called or the process ends.
    pub fn start(addr: &str, initial: MetricsSnapshot) -> std::io::Result<Exporter> {
        let listener = TcpListener::bind(addr)?;
        let addr = listener.local_addr()?;
        // A short accept timeout is what lets the thread notice `stop`.
        listener.set_nonblocking(true)?;

        let shared = Arc::new(Mutex::new(initial));
        let running = Arc::new(AtomicBool::new(true));

        let thread_shared = Arc::clone(&shared);
        let thread_running = Arc::clone(&running);
        std::thread::Builder::new()
            .name("metrics-exporter".into())
            .spawn(move || {
                while thread_running.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let snapshot = thread_shared
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .clone();
                            // A scrape that fails is a scrape that fails; it
                            // must never take the session down.
                            let _ = serve(stream, &snapshot);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        Err(_) => break,
                    }
                }
            })?;

        Ok(Exporter {
            shared,
            running,
            addr,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Publish a new snapshot. Cheap enough to call every frame.
    pub fn publish(&self, snapshot: &MetricsSnapshot) {
        let mut guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        *guard = snapshot.clone();
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

impl Drop for Exporter {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(mut stream: TcpStream, snapshot: &MetricsSnapshot) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let (status, body) = match path {
        "/metrics" => ("200 OK", render(snapshot)),
        "/" => (
            "200 OK",
            "rollback-netcode telemetry\nmetrics at /metrics\n".to_string(),
        ),
        _ => ("404 Not Found", "not found\n".to_string()),
    };

    write!(
        stream,
        "HTTP/1.1 {status}\r\n\
         Content-Type: text/plain; version=0.0.4; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Render the Prometheus text exposition format.
pub fn render(s: &MetricsSnapshot) -> String {
    let mut out = String::with_capacity(4096);

    let info = &s.info;
    out.push_str("# HELP rollback_session_info Static facts about this session.\n");
    out.push_str("# TYPE rollback_session_info gauge\n");
    out.push_str(&format!(
        "rollback_session_info{{simulation=\"{}\",profile=\"{}\",player=\"{}\",commit=\"{}\",core_sha256=\"{}\",rom_sha256=\"{}\",seed=\"{}\",input_delay=\"{}\",prediction_limit=\"{}\",state_history=\"{}\"}} 1\n",
        escape(&info.simulation),
        escape(&info.profile),
        escape(&info.player),
        escape(&info.app_commit),
        escape(&info.core_sha256),
        escape(&info.rom_sha256),
        info.seed,
        info.input_delay,
        info.prediction_limit,
        info.state_history,
    ));

    gauge(
        &mut out,
        "rollback_elapsed_seconds",
        "Session wall time.",
        s.elapsed_ms as f64 / 1000.0,
    );
    gauge(
        &mut out,
        "rollback_current_frame",
        "Next frame to simulate.",
        f64::from(s.frame),
    );
    gauge(
        &mut out,
        "rollback_confirmed_frame",
        "Highest frame with both inputs known.",
        f64::from(s.confirmed_frame),
    );
    gauge(
        &mut out,
        "rollback_prediction_depth",
        "Frames currently speculated ahead of the peer.",
        f64::from(s.prediction_depth),
    );
    gauge(
        &mut out,
        "rollback_desync",
        "1 if a confirmed checksum mismatched.",
        u8::from(s.desync).into(),
    );

    // --- per-peer session counters ---
    let local = &s.local;
    counter_peer(
        &mut out,
        "rollback_frames_presented_total",
        "Frames shown to the player.",
        "local",
        local.frames_presented as f64,
        true,
    );
    counter_peer(
        &mut out,
        "rollback_frames_resimulated_total",
        "Frames replayed during rollbacks.",
        "local",
        local.frames_resimulated as f64,
        true,
    );
    counter_peer(
        &mut out,
        "rollback_rollbacks_total",
        "Rollbacks performed.",
        "local",
        local.rollbacks as f64,
        true,
    );
    counter_peer(
        &mut out,
        "rollback_predicted_frames_total",
        "Frames first simulated on a guess.",
        "local",
        local.predicted_frames as f64,
        true,
    );
    counter_peer(
        &mut out,
        "rollback_mispredicted_frames_total",
        "Guesses that turned out wrong.",
        "local",
        local.mispredicted_frames as f64,
        true,
    );
    counter_peer(
        &mut out,
        "rollback_stalls_total",
        "Times the prediction window filled up.",
        "local",
        local.stalls as f64,
        true,
    );
    counter_peer(
        &mut out,
        "rollback_checksums_compared_total",
        "Confirmed-frame checksums compared.",
        "local",
        local.checksums_compared as f64,
        true,
    );
    gauge_peer(
        &mut out,
        "rollback_max_depth_frames",
        "Deepest rollback so far.",
        "local",
        f64::from(local.max_rollback_depth),
        true,
    );
    gauge_peer(
        &mut out,
        "rollback_prediction_accuracy",
        "Fraction of guesses that held up.",
        "local",
        local.prediction_accuracy(),
        true,
    );
    gauge_peer(
        &mut out,
        "rollback_state_bytes",
        "Size of the most recent saved state.",
        "local",
        local.state_bytes_last as f64,
        true,
    );

    // --- link ---
    let link = &s.link;
    gauge_peer(
        &mut out,
        "rollback_srtt_seconds",
        "Smoothed round-trip time.",
        "local",
        link.srtt_ms() / 1000.0,
        true,
    );
    gauge_peer(
        &mut out,
        "rollback_rttvar_seconds",
        "Round-trip time variation.",
        "local",
        link.rttvar_ms() / 1000.0,
        true,
    );
    gauge_peer(
        &mut out,
        "rollback_loss_ratio",
        "Inferred inbound datagram loss.",
        "local",
        link.loss_ratio(),
        true,
    );
    counter_peer(
        &mut out,
        "rollback_packets_sent_total",
        "Datagrams sent.",
        "local",
        link.packets_sent as f64,
        true,
    );
    counter_peer(
        &mut out,
        "rollback_packets_received_total",
        "Datagrams received and authenticated.",
        "local",
        link.packets_received as f64,
        true,
    );
    counter_peer(
        &mut out,
        "rollback_bytes_sent_total",
        "Bytes sent.",
        "local",
        link.bytes_sent as f64,
        true,
    );
    counter_peer(
        &mut out,
        "rollback_bytes_received_total",
        "Bytes received.",
        "local",
        link.bytes_received as f64,
        true,
    );
    counter_peer(
        &mut out,
        "rollback_inferred_lost_total",
        "Datagrams the peer sent that never arrived.",
        "local",
        link.inferred_lost() as f64,
        true,
    );
    counter_peer(
        &mut out,
        "rollback_duplicates_total",
        "Duplicate datagrams received.",
        "local",
        link.duplicates_received as f64,
        true,
    );
    counter_peer(
        &mut out,
        "rollback_reordered_total",
        "Out-of-order datagrams received.",
        "local",
        link.reordered_received as f64,
        true,
    );
    counter(
        &mut out,
        "rollback_auth_failures_total",
        "Datagrams that failed the HMAC check.",
        link.auth_failures as f64,
    );
    counter(
        &mut out,
        "rollback_malformed_total",
        "Datagrams that authenticated but did not parse.",
        link.malformed as f64,
    );
    gauge(
        &mut out,
        "rollback_send_bitrate_bps",
        "Outbound bitrate.",
        link.send_bitrate(s.elapsed_ms),
    );
    gauge(
        &mut out,
        "rollback_receive_bitrate_bps",
        "Inbound bitrate.",
        link.receive_bitrate(s.elapsed_ms),
    );

    // --- timings ---
    counter(
        &mut out,
        "rollback_advance_seconds_total",
        "Time inside advance_frame.",
        local.advance_nanos as f64 / 1e9,
    );
    counter(
        &mut out,
        "rollback_save_state_seconds_total",
        "Time inside save_state.",
        local.save_state_nanos as f64 / 1e9,
    );
    counter(
        &mut out,
        "rollback_load_state_seconds_total",
        "Time inside load_state.",
        local.load_state_nanos as f64 / 1e9,
    );

    // --- process ---
    counter(
        &mut out,
        "process_cpu_seconds_total",
        "Process CPU time.",
        s.process.cpu_seconds,
    );
    gauge(
        &mut out,
        "process_resident_memory_bytes",
        "Process resident set size.",
        s.process.resident_bytes as f64,
    );

    // --- the peer's own view, as it last reported it ---
    if let Some(r) = &s.remote {
        counter_peer(
            &mut out,
            "rollback_frames_presented_total",
            "",
            "remote",
            r.frames_presented as f64,
            false,
        );
        counter_peer(
            &mut out,
            "rollback_frames_resimulated_total",
            "",
            "remote",
            r.frames_resimulated as f64,
            false,
        );
        counter_peer(
            &mut out,
            "rollback_rollbacks_total",
            "",
            "remote",
            r.rollbacks as f64,
            false,
        );
        counter_peer(
            &mut out,
            "rollback_predicted_frames_total",
            "",
            "remote",
            r.predicted_frames as f64,
            false,
        );
        counter_peer(
            &mut out,
            "rollback_mispredicted_frames_total",
            "",
            "remote",
            r.mispredicted_frames as f64,
            false,
        );
        counter_peer(
            &mut out,
            "rollback_stalls_total",
            "",
            "remote",
            r.stalls as f64,
            false,
        );
        counter_peer(
            &mut out,
            "rollback_checksums_compared_total",
            "",
            "remote",
            r.checksums_compared as f64,
            false,
        );
        gauge_peer(
            &mut out,
            "rollback_max_depth_frames",
            "",
            "remote",
            f64::from(r.max_rollback_depth),
            false,
        );
        gauge_peer(
            &mut out,
            "rollback_prediction_accuracy",
            "",
            "remote",
            accuracy(r.predicted_frames, r.mispredicted_frames),
            false,
        );
        gauge_peer(
            &mut out,
            "rollback_state_bytes",
            "",
            "remote",
            f64::from(r.state_bytes_last),
            false,
        );
        gauge_peer(
            &mut out,
            "rollback_srtt_seconds",
            "",
            "remote",
            f64::from(r.srtt_micros) / 1e6,
            false,
        );
        gauge_peer(
            &mut out,
            "rollback_rttvar_seconds",
            "",
            "remote",
            f64::from(r.rttvar_micros) / 1e6,
            false,
        );
        counter_peer(
            &mut out,
            "rollback_packets_sent_total",
            "",
            "remote",
            r.packets_sent as f64,
            false,
        );
        counter_peer(
            &mut out,
            "rollback_packets_received_total",
            "",
            "remote",
            r.packets_received as f64,
            false,
        );
        counter_peer(
            &mut out,
            "rollback_bytes_sent_total",
            "",
            "remote",
            r.bytes_sent as f64,
            false,
        );
        counter_peer(
            &mut out,
            "rollback_bytes_received_total",
            "",
            "remote",
            r.bytes_received as f64,
            false,
        );
        counter_peer(
            &mut out,
            "rollback_inferred_lost_total",
            "",
            "remote",
            r.inferred_lost as f64,
            false,
        );
        counter_peer(
            &mut out,
            "rollback_duplicates_total",
            "",
            "remote",
            r.duplicates as f64,
            false,
        );
        counter_peer(
            &mut out,
            "rollback_reordered_total",
            "",
            "remote",
            r.reordered as f64,
            false,
        );
    }

    out
}

pub fn accuracy(predicted: u64, mispredicted: u64) -> f64 {
    if predicted == 0 {
        return 1.0;
    }
    predicted.saturating_sub(mispredicted) as f64 / predicted as f64
}

fn header(out: &mut String, name: &str, help: &str, kind: &str) {
    if !help.is_empty() {
        out.push_str(&format!("# HELP {name} {help}\n# TYPE {name} {kind}\n"));
    }
}

fn gauge(out: &mut String, name: &str, help: &str, value: f64) {
    header(out, name, help, "gauge");
    out.push_str(&format!("{name} {}\n", number(value)));
}

fn counter(out: &mut String, name: &str, help: &str, value: f64) {
    header(out, name, help, "counter");
    out.push_str(&format!("{name} {}\n", number(value)));
}

fn gauge_peer(out: &mut String, name: &str, help: &str, peer: &str, value: f64, emit_header: bool) {
    if emit_header {
        header(out, name, help, "gauge");
    }
    out.push_str(&format!("{name}{{peer=\"{peer}\"}} {}\n", number(value)));
}

fn counter_peer(
    out: &mut String,
    name: &str,
    help: &str,
    peer: &str,
    value: f64,
    emit_header: bool,
) {
    if emit_header {
        header(out, name, help, "counter");
    }
    out.push_str(&format!("{name}{{peer=\"{peer}\"}} {}\n", number(value)));
}

/// Format a value the way Prometheus expects: no thousands separators, and
/// `NaN`/`Inf` spelled the way the parser accepts them.
fn number(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "+Inf" } else { "-Inf" }.to_string();
    }
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    format!("{v}")
}

/// Escape a label value: backslash, quote and newline, per the exposition spec.
fn escape(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::SessionInfo;
    use rollback_core::SimulationKind;

    fn snapshot() -> MetricsSnapshot {
        let mut info = SessionInfo::new(SimulationKind::Arena, "combined", "p1");
        info.app_commit = "abc123".into();
        info.seed = 42;
        let mut s = MetricsSnapshot::new(info);
        s.elapsed_ms = 180_000;
        s.frame = 10_800;
        s.local.frames_presented = 10_800;
        s.local.rollbacks = 421;
        s.local.predicted_frames = 4_000;
        s.local.mispredicted_frames = 1_000;
        s.link.packets_sent = 10_800;
        s.link.bytes_sent = 10_800 * 90;
        s.link.srtt_micros = 32_500;
        s
    }

    #[test]
    fn the_rendered_output_parses_as_prometheus_text() {
        let text = render(&snapshot());
        for line in text.lines() {
            if line.starts_with('#') {
                assert!(
                    line.starts_with("# HELP ") || line.starts_with("# TYPE "),
                    "bad comment line: {line}"
                );
                continue;
            }
            let (name, value) = line
                .rsplit_once(' ')
                .unwrap_or_else(|| panic!("bad line: {line}"));
            assert!(!name.is_empty(), "empty metric name in {line}");
            assert!(
                value.parse::<f64>().is_ok() || ["NaN", "+Inf", "-Inf"].contains(&value),
                "unparseable value in {line}"
            );
        }
    }

    #[test]
    fn every_metric_has_a_help_and_a_type_line() {
        let text = render(&snapshot());
        let mut declared = std::collections::HashSet::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("# TYPE ") {
                declared.insert(rest.split_whitespace().next().unwrap().to_string());
            }
        }
        for line in text.lines() {
            if line.starts_with('#') {
                continue;
            }
            let name = line
                .split(['{', ' '])
                .next()
                .expect("every sample line has a name");
            assert!(declared.contains(name), "{name} has no TYPE line");
        }
    }

    #[test]
    fn the_session_info_label_set_survives_quotes_and_backslashes() {
        let mut info = SessionInfo::new(SimulationKind::Sfa3, "natural", "p2");
        info.app_commit = "he said \"hi\"\\n".into();
        let text = render(&MetricsSnapshot::new(info));
        let line = text
            .lines()
            .find(|l| l.starts_with("rollback_session_info"))
            .unwrap();
        assert!(line.contains("he said \\\"hi\\\""), "got {line}");
    }

    #[test]
    fn remote_metrics_only_appear_once_the_peer_has_reported() {
        let mut s = snapshot();
        assert!(!render(&s).contains("peer=\"remote\""));
        s.remote = Some(rollback_net::TelemetrySummary {
            frames_presented: 10_799,
            rollbacks: 400,
            ..Default::default()
        });
        let text = render(&s);
        assert!(text.contains("rollback_rollbacks_total{peer=\"remote\"} 400"));
        assert!(text.contains("rollback_rollbacks_total{peer=\"local\"} 421"));
    }

    #[test]
    fn integers_render_without_scientific_notation() {
        assert_eq!(number(10_800.0), "10800");
        assert_eq!(number(0.0), "0");
        assert_eq!(number(-3.0), "-3");
        assert!(number(0.0325).starts_with("0.0325"));
        assert_eq!(number(f64::NAN), "NaN");
        assert_eq!(number(f64::INFINITY), "+Inf");
    }

    #[test]
    fn accuracy_is_one_when_nothing_was_predicted() {
        assert_eq!(accuracy(0, 0), 1.0);
        assert_eq!(accuracy(4, 1), 0.75);
        assert_eq!(accuracy(4, 9), 0.0);
    }

    #[test]
    fn the_exporter_serves_metrics_over_http() {
        use std::io::Read;
        let exporter = Exporter::start("127.0.0.1:0", snapshot()).unwrap();
        let addr = exporter.addr();

        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("text/plain; version=0.0.4"));
        assert!(response.contains("rollback_rollbacks_total{peer=\"local\"} 421"));
    }

    #[test]
    fn an_unknown_path_is_a_404() {
        use std::io::Read;
        let exporter = Exporter::start("127.0.0.1:0", snapshot()).unwrap();
        let mut stream = TcpStream::connect(exporter.addr()).unwrap();
        stream.write_all(b"GET /admin HTTP/1.1\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 404 Not Found"));
    }

    #[test]
    fn published_snapshots_replace_the_previous_one() {
        use std::io::Read;
        let exporter = Exporter::start("127.0.0.1:0", snapshot()).unwrap();
        let mut updated = snapshot();
        updated.local.rollbacks = 999;
        exporter.publish(&updated);

        let mut stream = TcpStream::connect(exporter.addr()).unwrap();
        stream.write_all(b"GET /metrics HTTP/1.1\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.contains("rollback_rollbacks_total{peer=\"local\"} 999"));
    }
}
