//! Pieces both binaries need: build identity, resolve artefacts, name sessions.

use std::path::Path;

use rollback_core::{PlayerHandle, SessionConfig, SimulationKind};
use rollback_net::{PeerIdentity, PROTOCOL_VERSION};

/// Git commit this binary was built from, captured by `build.rs`.
pub const APP_COMMIT: &str = env!("ROLLBACK_APP_COMMIT");

/// The commit as the fixed-width field the wire format carries.
pub fn app_commit_bytes() -> [u8; 20] {
    let mut out = [0u8; 20];
    let bytes = APP_COMMIT.as_bytes();
    let n = bytes.len().min(20);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

/// Build the identity this peer presents at handshake time.
pub fn identity(
    config: &SessionConfig,
    player: PlayerHandle,
    core_hash: [u8; 32],
    rom_hash: [u8; 32],
) -> PeerIdentity {
    PeerIdentity {
        protocol_version: PROTOCOL_VERSION,
        simulation: config.simulation,
        player,
        app_commit: app_commit_bytes(),
        config_hash: config.compatibility_hash(),
        seed: config.seed,
        core_hash,
        rom_hash,
    }
}

/// A filesystem-safe, sortable name for a session's log file.
pub fn session_name(
    simulation: SimulationKind,
    profile: &str,
    player: PlayerHandle,
    mode: &str,
) -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let player = match player {
        PlayerHandle::P1 => "p1",
        PlayerHandle::P2 => "p2",
    };
    format!(
        "{stamp}-{}-{}-{}-{}",
        simulation.as_str(),
        sanitise(profile),
        player,
        sanitise(mode)
    )
}

/// Keep only characters that are safe in a filename on every platform.
fn sanitise(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Resolve the session key, in order of preference.
///
/// The key never appears on a command line: an argument is visible in `ps` to
/// every user on the box. It comes from the environment (set by `just`, which
/// reads it from SSM) or from a file readable only by the owner.
pub fn session_key_from_env() -> Result<rollback_net::Authenticator, String> {
    if let Ok(hex) = std::env::var("ROLLBACK_SESSION_KEY") {
        return rollback_net::Authenticator::from_hex(&hex)
            .map_err(|e| format!("ROLLBACK_SESSION_KEY is invalid: {e}"));
    }
    if let Ok(path) = std::env::var("ROLLBACK_SESSION_KEY_FILE") {
        let hex = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read ROLLBACK_SESSION_KEY_FILE '{path}': {e}"))?;
        return rollback_net::Authenticator::from_hex(&hex)
            .map_err(|e| format!("session key file '{path}' is invalid: {e}"));
    }
    Err("no session key: set ROLLBACK_SESSION_KEY or ROLLBACK_SESSION_KEY_FILE".to_string())
}

/// Hash an optional artefact, returning the "not applicable" digest when the
/// path is absent.
///
/// Streamed rather than read whole: the FBNeo core is 90 MB and there is no
/// reason to hold it in memory to hash it.
pub fn hash_or_absent(path: Option<&Path>) -> std::io::Result<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let Some(path) = path else {
        return Ok([0u8; 32]);
    };
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

/// Render a digest as lowercase hex, for logs and the metrics label set.
pub fn digest_hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hash everything the emulator loads as ROM data: the game, and the BIOS if
/// the game needs one.
///
/// The handshake carries one `rom_hash`, and it has to cover *all* of it. A Neo
/// Geo game is only half the code that runs -- `neogeo.zip` supplies the BIOS
/// that boots it, and two peers with different BIOS revisions would pass the
/// handshake and then desync during the boot sequence, before either player had
/// touched a button. That failure looks exactly like a bug in the rollback and
/// is nothing of the sort, so the cheap fix is to refuse it at the door.
///
/// Domain-separated by role so that swapping the two files is a different hash.
pub fn hash_rom_set(rom: Option<&Path>, bios: Option<&Path>) -> std::io::Result<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"rom\0");
    hasher.update(hash_or_absent(rom)?);
    hasher.update(b"bios\0");
    hasher.update(hash_or_absent(bios)?);
    Ok(hasher.finalize().into())
}

/// Where the BIOS for `simulation` lives, given the system directory, or `None`
/// when this simulation does not use one.
pub fn bios_path(simulation: SimulationKind, system_dir: &Path) -> Option<std::path::PathBuf> {
    match simulation {
        // Neo Geo drivers pull in the `neogeo` romset; FBNeo looks for it
        // beside the game and in the system directory, and this lab puts it in
        // the system directory so both peers ship it the same way.
        SimulationKind::LastBlade2 => Some(system_dir.join("neogeo.zip")),
        SimulationKind::Arena | SimulationKind::Sfa3 => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commit_is_captured_and_fits_the_wire_field() {
        assert!(!APP_COMMIT.is_empty());
        let bytes = app_commit_bytes();
        assert_eq!(bytes.len(), 20);
        assert_eq!(
            &bytes[..8],
            &APP_COMMIT.as_bytes()[..8.min(APP_COMMIT.len())]
        );
    }

    #[test]
    fn identities_of_two_peers_from_the_same_build_are_compatible() {
        let config = SessionConfig::default();
        let p1 = identity(&config, PlayerHandle::P1, [0; 32], [0; 32]);
        let p2 = identity(&config, PlayerHandle::P2, [0; 32], [0; 32]);
        assert_eq!(p1.compatible_with(&p2), Ok(()));
    }

    #[test]
    fn a_different_seed_makes_the_identities_incompatible() {
        let p1 = identity(
            &SessionConfig::default(),
            PlayerHandle::P1,
            [0; 32],
            [0; 32],
        );
        let p2 = identity(
            &SessionConfig {
                seed: 999,
                ..Default::default()
            },
            PlayerHandle::P2,
            [0; 32],
            [0; 32],
        );
        assert!(p1.compatible_with(&p2).is_err());
    }

    #[test]
    fn session_names_are_filesystem_safe() {
        let name = session_name(
            SimulationKind::Sfa3,
            "jitter 30/15",
            PlayerHandle::P2,
            "play",
        );
        assert!(name.contains("sfa3"));
        assert!(name.contains("p2"));
        assert!(name.ends_with("play"));
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "unsafe characters in {name}"
        );
    }

    #[test]
    fn a_missing_artefact_hashes_to_the_absent_digest() {
        assert_eq!(hash_or_absent(None).unwrap(), [0u8; 32]);
        assert_eq!(digest_hex(&[0u8; 32]), "0".repeat(64));
    }

    #[test]
    fn hashing_a_real_file_matches_the_known_digest() {
        let path = std::env::temp_dir().join(format!("rollback-app-hash-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        let digest = hash_or_absent(Some(&path)).unwrap();
        assert_eq!(
            digest_hex(&digest),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn the_session_key_is_never_taken_from_a_command_line() {
        // Not a behaviour test so much as a guard: if someone adds a `--key`
        // flag, this comment and the function above are where to argue about it.
        let err = {
            // SAFETY-free: just clearing our own process environment.
            std::env::remove_var("ROLLBACK_SESSION_KEY");
            std::env::remove_var("ROLLBACK_SESSION_KEY_FILE");
            session_key_from_env().unwrap_err()
        };
        assert!(err.contains("ROLLBACK_SESSION_KEY"));
    }
}
