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
pub mod sfa3;

pub use core::{to_retropad, CoreError, LibretroCore, LibretroSimulation};
pub use hash::{file_sha256, to_hex, to_short_hex, ABSENT};
pub use host::{set_directories, set_options, VideoFrame};
pub use sfa3::{Macro, Sfa3Bot, Sfa3Director, Step};

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
    // Diagnostic input can open a service menu on a stray button combination.
    ("fbneo-diagnostic-input", "Hold Start"),
];
