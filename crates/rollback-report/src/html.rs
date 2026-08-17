//! A self-contained HTML report.
//!
//! No CDN, no external stylesheet, no JavaScript library, no fonts to fetch:
//! the file has to be readable from a laptop with the Wi-Fi off, months after
//! the AWS account it describes has been torn down. Charts are inline SVG for
//! the same reason.

use std::collections::BTreeMap;

use crate::parse::{Session, SessionSummary};

const CHART_W: f64 = 520.0;
const CHART_H: f64 = 150.0;
const PAD: f64 = 28.0;

pub fn render(sessions: &[Session], generated_at: &str) -> String {
    let mut out = String::with_capacity(64 * 1024);
    out.push_str("<h1>Rollback Netcode &mdash; relatório de sessões</h1>\n");
    out.push_str(&format!(
        "<p class=\"meta\">Gerado em {}. {} sessão(ões).</p>\n",
        escape(generated_at),
        sessions.len()
    ));

    if sessions.is_empty() {
        out.push_str(
            "<p class=\"warn\">Nenhum log encontrado. Rode <code>just bench</code> primeiro.</p>\n",
        );
        return wrap(&out);
    }

    out.push_str(&overview(sessions));

    // One section per (simulation, mode, profile), so the two peers of a run
    // sit next to each other -- the comparison the lab is actually about.
    let mut groups: BTreeMap<(String, String, String), Vec<&Session>> = BTreeMap::new();
    for session in sessions {
        groups
            .entry(session.summary.group())
            .or_default()
            .push(session);
    }

    for ((simulation, mode, profile), group) in &groups {
        out.push_str(&format!(
            "<h2>{} &middot; {} &middot; perfil {}</h2>\n",
            escape(simulation),
            escape(mode),
            escape(profile)
        ));
        out.push_str(&peer_table(group));
        for session in group {
            if session.series.len() >= 2 {
                out.push_str(&format!(
                    "<h3>{} &mdash; séries temporais</h3>\n",
                    escape(&session.summary.player)
                ));
                out.push_str("<div class=\"charts\">");
                out.push_str(&chart(
                    "Rollbacks acumulados",
                    &session
                        .series
                        .iter()
                        .map(|p| (p.t_s, p.rollbacks as f64))
                        .collect::<Vec<_>>(),
                    "#c8503f",
                    "",
                ));
                out.push_str(&chart(
                    "RTT suavizado",
                    &session
                        .series
                        .iter()
                        .map(|p| (p.t_s, p.srtt_ms))
                        .collect::<Vec<_>>(),
                    "#3f7fc8",
                    " ms",
                ));
                out.push_str(&chart(
                    "Profundidade de previsão",
                    &session
                        .series
                        .iter()
                        .map(|p| (p.t_s, p.prediction_depth as f64))
                        .collect::<Vec<_>>(),
                    "#c8a03f",
                    " frames",
                ));
                out.push_str("</div>\n");
            }
        }
    }

    out.push_str(&caveats());
    wrap(&out)
}

/// One row per profile, aggregated across peers: the headline comparison.
fn overview(sessions: &[Session]) -> String {
    let mut by_profile: BTreeMap<String, Vec<&SessionSummary>> = BTreeMap::new();
    for s in sessions {
        by_profile
            .entry(s.summary.profile.clone())
            .or_default()
            .push(&s.summary);
    }

    let mut out = String::from("<h2>Visão geral por perfil</h2>\n<table>\n<tr>");
    for h in [
        "Perfil",
        "Sessões",
        "RTT médio",
        "Perda",
        "Rollbacks/min",
        "Prof. média",
        "Prof. máx",
        "Acurácia",
        "Stalls",
        "Desync",
    ] {
        out.push_str(&format!("<th>{h}</th>"));
    }
    out.push_str("</tr>\n");

    for (profile, group) in &by_profile {
        let n = group.len() as f64;
        let mean = |f: &dyn Fn(&SessionSummary) -> f64| -> f64 {
            group.iter().map(|s| f(s)).sum::<f64>() / n
        };
        let rollbacks_per_min = mean(&|s| {
            if s.duration_s <= 0.0 {
                0.0
            } else {
                s.rollbacks as f64 * 60.0 / s.duration_s
            }
        });
        let desync = group.iter().any(|s| s.desync);

        out.push_str(&format!(
            "<tr><td><b>{}</b></td><td>{}</td><td>{:.1} ms</td><td>{:.2}%</td><td>{:.1}</td><td>{:.2}</td><td>{}</td><td>{:.1}%</td><td>{:.0}</td><td class=\"{}\">{}</td></tr>\n",
            escape(profile),
            group.len(),
            mean(&|s| s.srtt_ms),
            mean(&|s| s.loss_ratio * 100.0),
            rollbacks_per_min,
            mean(&|s| s.mean_rollback_depth()),
            group.iter().map(|s| s.max_rollback_depth).max().unwrap_or(0),
            mean(&|s| s.prediction_accuracy() * 100.0),
            mean(&|s| s.stalls as f64),
            if desync { "bad" } else { "good" },
            if desync { "SIM" } else { "não" },
        ));
    }
    out.push_str("</table>\n");
    out
}

/// A labelled row of the peer table: a heading and how to render one peer's cell.
type PeerRow = (&'static str, Box<dyn Fn(&SessionSummary) -> String>);

/// The two peers of one run, side by side.
fn peer_table(group: &[&Session]) -> String {
    let rows: Vec<PeerRow> = vec![
        ("Sessão", Box::new(|s: &SessionSummary| escape(&s.name))),
        (
            "Commit",
            Box::new(|s: &SessionSummary| escape(&s.commit[..s.commit.len().min(12)])),
        ),
        (
            "Duração",
            Box::new(|s: &SessionSummary| format!("{:.1} s", s.duration_s)),
        ),
        (
            "Frames apresentados",
            Box::new(|s: &SessionSummary| s.frames_presented.to_string()),
        ),
        (
            "FPS efetivo",
            Box::new(|s: &SessionSummary| format!("{:.2}", s.effective_fps())),
        ),
        (
            "Frames re-simulados",
            Box::new(|s: &SessionSummary| s.frames_resimulated.to_string()),
        ),
        (
            "Trabalho extra",
            Box::new(|s: &SessionSummary| format!("{:.1}%", s.resimulation_overhead() * 100.0)),
        ),
        (
            "Rollbacks",
            Box::new(|s: &SessionSummary| s.rollbacks.to_string()),
        ),
        (
            "Profundidade média",
            Box::new(|s: &SessionSummary| format!("{:.2}", s.mean_rollback_depth())),
        ),
        (
            "Profundidade máxima",
            Box::new(|s: &SessionSummary| s.max_rollback_depth.to_string()),
        ),
        (
            "Frames previstos",
            Box::new(|s: &SessionSummary| s.predicted_frames.to_string()),
        ),
        (
            "Previsões erradas",
            Box::new(|s: &SessionSummary| s.mispredicted_frames.to_string()),
        ),
        (
            "Acurácia da previsão",
            Box::new(|s: &SessionSummary| format!("{:.2}%", s.prediction_accuracy() * 100.0)),
        ),
        (
            "Stalls",
            Box::new(|s: &SessionSummary| s.stalls.to_string()),
        ),
        (
            "Checksums comparados",
            Box::new(|s: &SessionSummary| s.checksums_compared.to_string()),
        ),
        (
            "Tamanho do estado",
            Box::new(|s: &SessionSummary| format!("{} B", s.state_bytes)),
        ),
        (
            "RTT suavizado",
            Box::new(|s: &SessionSummary| format!("{:.1} ms", s.srtt_ms)),
        ),
        (
            "Variação do RTT",
            Box::new(|s: &SessionSummary| format!("{:.1} ms", s.rttvar_ms)),
        ),
        (
            "Perda inferida",
            Box::new(|s: &SessionSummary| format!("{:.2}%", s.loss_ratio * 100.0)),
        ),
        (
            "Duplicados",
            Box::new(|s: &SessionSummary| s.duplicates.to_string()),
        ),
        (
            "Reordenados",
            Box::new(|s: &SessionSummary| s.reordered.to_string()),
        ),
        (
            "Bitrate de envio",
            Box::new(|s: &SessionSummary| format!("{:.1} kbit/s", s.send_bitrate() / 1000.0)),
        ),
        (
            "CPU",
            Box::new(|s: &SessionSummary| format!("{:.1} s", s.cpu_seconds)),
        ),
        (
            "Memória residente",
            Box::new(|s: &SessionSummary| format!("{:.0} MB", s.resident_bytes as f64 / 1048576.0)),
        ),
        (
            "Log completo",
            Box::new(|s: &SessionSummary| {
                if s.complete {
                    "sim".into()
                } else {
                    "NÃO".into()
                }
            }),
        ),
        (
            "Desync",
            Box::new(|s: &SessionSummary| {
                if s.desync {
                    "SIM".into()
                } else {
                    "não".into()
                }
            }),
        ),
    ];

    let mut out = String::from("<table class=\"peers\">\n<tr><th>Métrica</th>");
    for session in group {
        out.push_str(&format!("<th>{}</th>", escape(&session.summary.player)));
    }
    out.push_str("</tr>\n");

    for (label, value) in rows {
        out.push_str(&format!("<tr><td>{label}</td>"));
        for session in group {
            out.push_str(&format!("<td>{}</td>", value(&session.summary)));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>\n");
    out
}

/// A single-series line chart as inline SVG.
fn chart(title: &str, points: &[(f64, f64)], colour: &str, unit: &str) -> String {
    if points.len() < 2 {
        return String::new();
    }
    let (x_min, x_max) = extent(points.iter().map(|p| p.0));
    let (_, y_max_raw) = extent(points.iter().map(|p| p.1));
    // Always anchor the y axis at zero: a chart of counters that starts at the
    // first sample makes a flat series look dramatic.
    let y_max = if y_max_raw <= 0.0 { 1.0 } else { y_max_raw };
    let x_span = (x_max - x_min).max(1e-9);

    // Plot area: the box inside the axes. `sy` grows downward in SVG, so the
    // baseline is at `CHART_H - PAD` and a full-scale value sits `plot_h` above it.
    let plot_w = CHART_W - PAD * 1.5;
    let plot_h = CHART_H - PAD * 1.5;
    let sx = |x: f64| PAD + (x - x_min) / x_span * plot_w;
    let sy = |y: f64| CHART_H - PAD - (y / y_max) * plot_h;

    let path: String = points
        .iter()
        .enumerate()
        .map(|(i, (x, y))| {
            format!(
                "{}{:.1},{:.1}",
                if i == 0 { "M" } else { "L" },
                sx(*x),
                sy(*y)
            )
        })
        .collect::<Vec<_>>()
        .join(" ");

    format!(
        "<figure class=\"chart\"><figcaption>{title}</figcaption>\
<svg viewBox=\"0 0 {CHART_W} {CHART_H}\" role=\"img\" aria-label=\"{title}\">\
<line x1=\"{PAD}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" class=\"axis\"/>\
<line x1=\"{PAD}\" y1=\"{:.1}\" x2=\"{PAD}\" y2=\"{:.1}\" class=\"axis\"/>\
<path d=\"{path}\" fill=\"none\" stroke=\"{colour}\" stroke-width=\"2\"/>\
<text x=\"{:.1}\" y=\"12\" class=\"tick\">{:.1}{unit}</text>\
<text x=\"{PAD}\" y=\"{:.1}\" class=\"tick\">{:.0} s</text>\
<text x=\"{:.1}\" y=\"{:.1}\" class=\"tick end\">{:.0} s</text>\
</svg></figure>",
        CHART_H - PAD,
        CHART_W - PAD * 0.5,
        CHART_H - PAD,
        PAD * 0.5,
        CHART_H - PAD,
        PAD,
        y_max,
        CHART_H - PAD + 14.0,
        x_min,
        CHART_W - PAD * 1.5,
        CHART_H - PAD + 14.0,
        x_max,
    )
}

fn extent(values: impl Iterator<Item = f64>) -> (f64, f64) {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for v in values {
        min = min.min(v);
        max = max.max(v);
    }
    if !min.is_finite() || !max.is_finite() {
        return (0.0, 1.0);
    }
    (min, max)
}

fn caveats() -> String {
    String::from(
        "<h2>Como ler estes números</h2>\n<ul class=\"caveats\">\
<li><b>Não há latência unidirecional.</b> Os dois peers não compartilham relógio; \
o RTT precisa de um relógio só e é o que está reportado.</li>\
<li><b>A perda é inferida</b> a partir das lacunas na sequência do peer. Um datagrama \
atrasado aparece como perda até chegar, e então a estimativa se corrige sozinha.</li>\
<li><b>Frames apresentados diferem entre os peers</b> por design: cada lado desenha o \
próprio presente. O que precisa bater são os checksums dos frames confirmados.</li>\
<li><b>O perfil de rede é aplicado na saída</b> de cada peer. Um experimento simétrico \
tem o mesmo perfil dos dois lados, e o RTT vê aproximadamente o dobro do atraso configurado.</li>\
<li><b>Desync = sessão inválida.</b> Qualquer linha marcada com desync deve ser \
investigada, não interpretada.</li></ul>\n",
    )
}

fn wrap(body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"pt-BR\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Rollback Netcode — relatório</title><style>{}</style></head><body>{body}</body></html>\n",
        CSS
    )
}

const CSS: &str = "
:root { color-scheme: light dark; --fg: #1a1c22; --bg: #fbfbfd; --line: #d8dae2;
        --muted: #5a6070; --good: #2f7a4f; --bad: #b03024; --head: #eef0f6; }
@media (prefers-color-scheme: dark) {
  :root { --fg: #e6e8ee; --bg: #14161c; --line: #333846; --muted: #99a0b0;
          --good: #6ecf95; --bad: #f0736a; --head: #1e2129; }
}
body { margin: 0 auto; padding: 2rem 1.25rem 4rem; max-width: 1100px; background: var(--bg);
       color: var(--fg); font: 15px/1.55 system-ui, -apple-system, 'Segoe UI', sans-serif; }
h1 { font-size: 1.6rem; margin: 0 0 .25rem; }
h2 { font-size: 1.15rem; margin: 2.25rem 0 .5rem; border-bottom: 1px solid var(--line);
     padding-bottom: .3rem; }
h3 { font-size: .95rem; margin: 1.25rem 0 .4rem; color: var(--muted); font-weight: 600; }
.meta { color: var(--muted); margin: 0 0 1rem; }
.warn { color: var(--bad); }
table { border-collapse: collapse; width: 100%; margin: .5rem 0 1rem; font-size: .87rem;
        display: block; overflow-x: auto; }
th, td { border: 1px solid var(--line); padding: .3rem .55rem; text-align: right;
         white-space: nowrap; }
th { background: var(--head); font-weight: 600; }
th:first-child, td:first-child { text-align: left; }
tr:nth-child(even) td { background: color-mix(in srgb, var(--head) 45%, transparent); }
.good { color: var(--good); }
.bad { color: var(--bad); font-weight: 700; }
.charts { display: flex; flex-wrap: wrap; gap: 1rem; }
.chart { margin: 0; flex: 1 1 320px; min-width: 0; }
.chart figcaption { font-size: .8rem; color: var(--muted); margin-bottom: .2rem; }
.chart svg { width: 100%; height: auto; max-width: 100%; }
.axis { stroke: var(--line); stroke-width: 1; }
.tick { fill: var(--muted); font-size: 10px; }
.tick.end { text-anchor: end; }
.caveats { color: var(--muted); }
.caveats b { color: var(--fg); }
code { background: var(--head); padding: .1rem .3rem; border-radius: 3px; }
";

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::TimePoint;

    fn session(player: &str, profile: &str, desync: bool) -> Session {
        Session {
            summary: SessionSummary {
                name: format!("1700000000-arena-{profile}-{player}-bench"),
                simulation: "arena".into(),
                mode: "bench".into(),
                profile: profile.into(),
                player: player.into(),
                commit: "abcdef0123456789".into(),
                duration_s: 180.0,
                frames_presented: 10_800,
                frames_resimulated: 1_684,
                rollbacks: 421,
                max_rollback_depth: 6,
                predicted_frames: 4_000,
                mispredicted_frames: 1_000,
                checksums_compared: 178,
                state_bytes: 204,
                srtt_ms: 41.0,
                rttvar_ms: 7.0,
                loss_ratio: 0.0185,
                bytes_sent: 972_000,
                complete: true,
                desync,
                ..Default::default()
            },
            series: (0..10)
                .map(|i| TimePoint {
                    t_s: i as f64,
                    rollbacks: i * 40,
                    srtt_ms: 40.0 + i as f64,
                    prediction_depth: 3,
                    loss_ratio: 0.02,
                })
                .collect(),
        }
    }

    #[test]
    fn the_report_is_self_contained() {
        let html = render(&[session("p1", "combined", false)], "2026-08-18");
        for forbidden in ["http://", "https://", "<script", "src=", "@import"] {
            assert!(
                !html.contains(forbidden),
                "report reaches outside itself via {forbidden}"
            );
        }
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<style>"));
    }

    #[test]
    fn both_peers_appear_in_one_table() {
        let html = render(
            &[
                session("p1", "combined", false),
                session("p2", "combined", false),
            ],
            "now",
        );
        assert!(html.contains("<th>p1</th>"));
        assert!(html.contains("<th>p2</th>"));
        // One section for the group, not one per peer.
        assert_eq!(html.matches("perfil combined").count(), 1);
    }

    #[test]
    fn every_profile_gets_an_overview_row() {
        let sessions: Vec<Session> = ["natural", "delay20", "jitter30", "loss2", "combined"]
            .iter()
            .map(|p| session("p1", p, false))
            .collect();
        let html = render(&sessions, "now");
        for p in ["natural", "delay20", "jitter30", "loss2", "combined"] {
            assert!(html.contains(&format!("<b>{p}</b>")), "{p} missing");
        }
    }

    #[test]
    fn a_desync_is_called_out_loudly() {
        let html = render(&[session("p1", "combined", true)], "now");
        assert!(html.contains("class=\"bad\">SIM"));
    }

    #[test]
    fn an_empty_run_produces_a_readable_page() {
        let html = render(&[], "now");
        assert!(html.contains("Nenhum log encontrado"));
        assert!(html.contains("just bench"));
    }

    #[test]
    fn charts_are_inline_svg_with_finite_coordinates() {
        let html = render(&[session("p1", "combined", false)], "now");
        assert!(html.contains("<svg viewBox="));

        // Check the actual path data rather than searching the page for "NaN"
        // or "inf": the prose legitimately contains "inferida".
        let mut paths = 0;
        for chunk in html.split("<path d=\"").skip(1) {
            paths += 1;
            let data = chunk.split('"').next().unwrap();
            for token in data.split_whitespace() {
                let (x, y) = token
                    .trim_start_matches(['M', 'L'])
                    .split_once(',')
                    .unwrap_or_else(|| panic!("malformed path segment {token:?}"));
                for value in [x, y] {
                    let n: f64 = value
                        .parse()
                        .unwrap_or_else(|_| panic!("non-numeric coordinate {value:?}"));
                    assert!(n.is_finite(), "non-finite coordinate in {data:?}");
                    assert!((0.0..=CHART_W).contains(&n) || (0.0..=CHART_H).contains(&n));
                }
            }
        }
        assert_eq!(paths, 3, "expected three charts per session");
    }

    #[test]
    fn a_session_with_one_sample_draws_no_chart_rather_than_a_broken_one() {
        let mut s = session("p1", "natural", false);
        s.series.truncate(1);
        let html = render(&[s], "now");
        assert!(!html.contains("<svg"));
    }

    #[test]
    fn html_special_characters_are_escaped() {
        let mut s = session("p1", "natural", false);
        s.summary.profile = "<script>alert(1)</script>".into();
        let html = render(&[s], "now");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn the_caveats_explain_why_there_is_no_one_way_latency() {
        let html = render(&[session("p1", "natural", false)], "now");
        assert!(html.contains("latência unidirecional"));
        assert!(html.contains("perda é inferida"));
    }
}
