//! A deterministic, integer-only 2D fighting arena and its FSM bot.
//!
//! This is the *instrumented* half of the lab. The emulated game proves the
//! rollback works on a real game; the arena proves it works on something we can
//! open up, whose state is 204 bytes, whose checksum covers every field, and
//! whose 100 000-frame replay must produce the same hash in debug and release.

#![forbid(unsafe_code)]

pub mod arena;
pub mod bot;
pub mod codec;
pub mod fixed;

pub use arena::{
    Action, Arena, Fighter, FighterView, Projectile, FIGHTER_HALF_WIDTH, MAX_HEALTH,
    MAX_PROJECTILES, STAGE_MAX_X, STAGE_MIN_X, STATE_BYTES,
};
pub use bot::{ArenaBot, Plan};
