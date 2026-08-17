//! Rollback netcode core: prediction, state history, re-simulation, desync
//! detection. Transport-agnostic and simulation-agnostic on purpose -- the same
//! [`RollbackSession`] drives the instrumented 2D arena and Street Fighter
//! Alpha 3 running under the libretro core.
//!
//! ```
//! use rollback_core::{
//!     AdvanceOutcome, PlayerHandle, PlayerInput, RollbackSession, SessionConfig,
//!     testing::CounterSim,
//! };
//!
//! let mut session =
//!     RollbackSession::new(CounterSim::default(), SessionConfig::default(), PlayerHandle::P1)
//!         .unwrap();
//!
//! session.add_local_input(PlayerInput(0x01)).unwrap();
//! match session.advance().unwrap() {
//!     AdvanceOutcome::Advanced { frame, predicted } => {
//!         assert_eq!(frame, 0);
//!         assert!(!predicted, "frame 0 is covered by the input-delay pre-fill");
//!     }
//!     AdvanceOutcome::Stalled { .. } => unreachable!(),
//! }
//! ```

#![forbid(unsafe_code)]

pub mod config;
pub mod input;
pub mod rng;
pub mod session;
pub mod simulation;
pub mod stats;
pub mod testing;

pub use config::{
    ConfigError, Fnv1a, Frame, NetworkProfile, SessionConfig, SimulationKind, NULL_FRAME,
};
pub use input::{Button, PlayerHandle, PlayerInput};
pub use rng::DeterministicRng;
pub use session::{AdvanceOutcome, EndReason, RollbackSession, SessionError};
pub use simulation::{OutputMode, Simulation, SimulationError};
pub use stats::{SessionEvent, SessionStats};
