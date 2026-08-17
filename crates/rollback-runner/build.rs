//! Capture the git commit at build time.
//!
//! The handshake compares it between peers: two builds from different commits
//! may simulate differently even with identical configuration, and that is a
//! class of desync that is very hard to diagnose from the inside.

use std::process::Command;

fn main() {
    // Rebuild when HEAD moves, so the constant never goes stale.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-env-changed=ROLLBACK_APP_COMMIT");

    let commit = std::env::var("ROLLBACK_APP_COMMIT").ok().or_else(|| {
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_string())
    });

    // "unknown" is a legitimate value: a tarball build with no git available
    // still works, as long as *both* peers are in that situation -- which the
    // handshake then verifies for us.
    let commit = commit.unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=ROLLBACK_APP_COMMIT={commit}");
}
