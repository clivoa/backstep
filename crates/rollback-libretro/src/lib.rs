//! Hosting a libretro core as a rollback simulation.
//!
//! The rollback machinery in `rollback-core` does not care what it is
//! simulating -- it needs `save_state`, `load_state`, `advance_frame` and
//! `checksum`. libretro happens to offer exactly those through
//! `retro_serialize`, `retro_unserialize` and `retro_run`, which is why a real
//! arcade emulator can be dropped into the same session as the toy arena.
//!
//! Three things this module is careful about:
//!
//! * **Output during re-simulation.** A rollback replays up to eight frames
//!   inside one display frame. The host suppresses video and audio for those,
//!   so the player sees one corrected frame instead of a stutter and a burst of
//!   sound (see [`host::video_refresh`]).
//! * **Identical starting conditions.** FBNeo reads NVRAM and per-game settings
//!   from its system directory; a stale file on one peer is a desync before the
//!   first input. [`host::set_directories`] exists to point both peers at a
//!   clean, identical directory.
//! * **No ROM in CI.** [`core::LibretroCore`] is exercised against
//!   `fake-libretro-core`, a real `cdylib` implementing the same ABI, so the
//!   FFI path is tested on machines that have never seen a protected ROM.

pub mod core;
pub mod ffi;
pub mod hash;
pub mod host;
pub mod script;

pub use core::{to_retropad, CoreError, LibretroCore, LibretroSimulation};

/// Delete the per-game files FBNeo persists, so a session starts from a machine
/// in a known state.
///
/// Returns the paths actually removed.
///
/// This is not tidiness, it is a determinism requirement, and it was found the
/// hard way. FBNeo writes `<system>/fbneo/<romset>.fs` on unload -- the Neo Geo
/// memory card, which holds leftover *credits* among other things. A peer that
/// has run before boots with credits already inserted, reaches the title screen
/// at a different frame than a peer that has not, and the two boot scripts then
/// press Start at different moments in the attract loop. The result is two
/// machines in different menus, which the rollback faithfully reports as a
/// desync.
///
/// Both peers calling this before loading is what makes "same ROM, same inputs,
/// same result" actually true.
pub fn clear_persistent_state(
    system_dir: &std::path::Path,
    romset: &str,
) -> std::io::Result<Vec<std::path::PathBuf>> {
    let mut removed = Vec::new();
    // `.fs` is the memory card, `.nv` non-volatile RAM, `.hi` the hiscore file.
    for extension in ["fs", "nv", "hi"] {
        let path = system_dir
            .join("fbneo")
            .join(format!("{romset}.{extension}"));
        match std::fs::remove_file(&path) {
            Ok(()) => removed.push(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(removed)
}
pub use hash::{file_sha256, to_hex, to_short_hex, ABSENT};
pub use host::{set_directories, set_options, VideoFrame};
pub use script::{BootDirector, Game, Macro, ScriptedBot, Step};

/// Core options pinned for every session.
///
/// Anything that changes how many machine cycles a frame runs for would make
/// the two peers diverge, so the options that could do that are set explicitly
/// rather than left to whatever the core defaults to on that machine.
pub const PINNED_CORE_OPTIONS: &[(&str, &str)] = &[
    // Frame skipping would make `retro_run` advance a variable number of
    // frames. Rollback assumes exactly one.
    ("fbneo-frameskip", "0"),
    // CPU speed adjustment changes the cycle budget per frame.
    ("fbneo-cpu-speed-adjust", "100"),
    // Keep the emulated hardware region fixed: it changes the frame rate.
    ("fbneo-neogeo-mode", "DIPSWITCH"),
    // Move the service-menu trigger onto a combination the lab can never
    // produce. `to_retropad` never sets the shoulder buttons, so L+R is
    // unreachable by any input in this workspace.
    //
    // This is not paranoia, it is a bug that already happened: the value used
    // to be "Hold Start", and the boot script holds Start for twelve frames to
    // begin a match. FBNeo swallowed it as a diagnostic gesture, the game sat
    // in attract mode with five credits showing, and the script marched on
    // into a match that had never started.
    ("fbneo-diagnostic-input", "Disabled"),
];
