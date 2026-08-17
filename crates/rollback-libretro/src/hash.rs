//! SHA-256 of the artefacts both peers must agree on.
//!
//! The handshake compares the hash of the libretro core and of the ROM. Neither
//! file is ever sent over the link -- only the 32-byte digest -- so a peer
//! learns whether its counterpart has the same file without the lab
//! distributing anything.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

/// A file's SHA-256, streamed so a 90 MB core does not need 90 MB of RAM.
pub fn file_sha256(path: impl AsRef<Path>) -> std::io::Result<[u8; 32]> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

/// The all-zero digest, meaning "not applicable" (the arena has no core or ROM).
pub const ABSENT: [u8; 32] = [0u8; 32];

pub fn to_hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Short form for logs and the overlay.
pub fn to_short_hex(digest: &[u8; 32]) -> String {
    to_hex(digest)[..12].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(contents: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "rollback-hash-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents).unwrap();
        path
    }

    #[test]
    fn hashes_the_known_sha256_of_abc() {
        let path = temp_file(b"abc");
        let digest = file_sha256(&path).unwrap();
        assert_eq!(
            to_hex(&digest),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_zero_digest() {
        assert!(file_sha256("/nonexistent/rollback-lab/file").is_err());
    }

    #[test]
    fn hex_helpers_agree_with_each_other() {
        let digest = [0xDEu8; 32];
        assert_eq!(to_hex(&digest).len(), 64);
        assert_eq!(to_short_hex(&digest), "dededededede");
        assert_eq!(to_hex(&ABSENT), "0".repeat(64));
    }
}
