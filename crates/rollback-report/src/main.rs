//! Turn a directory of JSONL session logs into `summary.csv` and `report.html`.

mod html;
mod parse;
mod summary;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "rollback-report",
    about = "Build summary.csv and a self-contained HTML report from session logs."
)]
struct Args {
    /// Directory of `*.jsonl` session logs, from both peers.
    #[arg(long, default_value = "artifacts/logs")]
    logs: PathBuf,

    /// Where to write `summary.csv` and `report.html`.
    #[arg(long, default_value = "artifacts/report")]
    out: PathBuf,

    /// Exit non-zero if any session recorded a desync or ended incomplete.
    #[arg(long)]
    strict: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let sessions = parse::read_dir(&args.logs)
        .with_context(|| format!("reading session logs from {}", args.logs.display()))?;
    std::fs::create_dir_all(&args.out)
        .with_context(|| format!("creating {}", args.out.display()))?;

    let summaries: Vec<parse::SessionSummary> =
        sessions.iter().map(|s| s.summary.clone()).collect();

    let csv_path = args.out.join("summary.csv");
    std::fs::write(&csv_path, summary::render(&summaries))
        .with_context(|| format!("writing {}", csv_path.display()))?;

    let html_path = args.out.join("report.html");
    std::fs::write(&html_path, html::render(&sessions, &now_iso8601()))
        .with_context(|| format!("writing {}", html_path.display()))?;

    println!(
        "{} sessão(ões) lidas de {}",
        sessions.len(),
        args.logs.display()
    );
    println!("  {}", csv_path.display());
    println!("  {}", html_path.display());

    let desynced: Vec<&str> = summaries
        .iter()
        .filter(|s| s.desync)
        .map(|s| s.name.as_str())
        .collect();
    let incomplete: Vec<&str> = summaries
        .iter()
        .filter(|s| !s.complete)
        .map(|s| s.name.as_str())
        .collect();

    if !desynced.is_empty() {
        eprintln!("DESYNC em: {}", desynced.join(", "));
    }
    if !incomplete.is_empty() {
        eprintln!("logs incompletos: {}", incomplete.join(", "));
    }
    if args.strict && !(desynced.is_empty() && incomplete.is_empty()) {
        anyhow::bail!("--strict: há sessões com desync ou log incompleto");
    }
    Ok(())
}

/// Timestamp for the report header.
///
/// Formatted by hand from the Unix epoch: pulling in a date-time crate to print
/// one string would be the largest dependency in this binary.
fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    format_utc(secs)
}

/// Civil date from a Unix timestamp, using Howard Hinnant's `civil_from_days`.
fn format_utc(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        secs_of_day / 3_600,
        (secs_of_day / 60) % 60,
        secs_of_day % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_formats_correctly() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00 UTC");
    }

    #[test]
    fn known_timestamps_format_correctly() {
        // 2001-09-09T01:46:40Z, the famous 1e9 second mark.
        assert_eq!(format_utc(1_000_000_000), "2001-09-09 01:46:40 UTC");
        // A leap day, to catch off-by-one errors in the civil calendar.
        assert_eq!(format_utc(1_709_164_800), "2024-02-29 00:00:00 UTC");
        assert_eq!(format_utc(1_800_000_000), "2027-01-15 08:00:00 UTC");
    }

    #[test]
    fn a_pre_epoch_timestamp_does_not_panic() {
        assert_eq!(format_utc(-1), "1969-12-31 23:59:59 UTC");
    }

    #[test]
    fn the_current_time_is_plausible() {
        let now = now_iso8601();
        assert!(now.ends_with(" UTC"));
        let year: i32 = now[..4].parse().unwrap();
        assert!((2020..2100).contains(&year), "got {now}");
    }
}
