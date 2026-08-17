//! The session loop shared by the SDL2 client and the headless bot.
//!
//! Both peers run exactly the same code here; the only differences are who
//! dials whom, which player slot they take, and where the local input comes
//! from. Keeping the loop in one place is what makes "the two peers agree"
//! a property of one implementation rather than of two that look similar.
//!
//! # The order of operations in a frame, and why
//!
//! ```text
//! 1. receive        pull everything the socket has; apply remote inputs first,
//!                   so this frame's prediction starts from the freshest data
//!                   and any rollback happens before we add work on top of it
//! 2. would_stall?   if the prediction window is full, do no local work at all
//! 3. read input     the human's controller or the bot's FSM
//! 4. send           the input batch goes out *before* the frame is simulated,
//!                   so the peer gets it a simulation's worth of time earlier
//! 5. advance        simulate and present
//! 6. checksums      exchange any frame that just became final
//! 7. telemetry      publish, log, check the peer timeout
//! ```

#![forbid(unsafe_code)]

pub mod app;
pub mod handshake;
pub mod runner;

pub use app::{app_commit_bytes, digest_hex, hash_or_absent, identity, session_key_from_env, session_name, APP_COMMIT};
pub use handshake::{handshake, HandshakeError, Role};
pub use runner::{RunnerConfig, SessionRunner, StepOutcome};
