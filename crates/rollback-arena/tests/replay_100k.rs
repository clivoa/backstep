//! The determinism acceptance test: 100 000 frames, one checksum.
//!
//! Run in debug and in release. If the two disagree, something in the arena is
//! sensitive to optimisation level -- which in practice means floating point,
//! uninitialised memory, or arithmetic that overflows differently. All three
//! are desyncs waiting to happen on a peer with a different build profile,
//! which is exactly what the two ends of this lab are.
//!
//! The golden constant is the point. Without it the test only proves the arena
//! agrees with itself in this process; with it, a change that silently alters
//! simulation behaviour has to be acknowledged by updating a number.

use rollback_arena::{Arena, ArenaBot};
use rollback_core::{OutputMode, PlayerInput, Simulation};

const FRAMES: usize = 100_000;

/// Checksum after `FRAMES` frames of the scripted replay below.
///
/// Update deliberately: a change here means the simulation changed, and every
/// peer must be rebuilt before they can play together again.
const GOLDEN_SCRIPTED: u64 = 0xf594_92aa_1a1b_d8cf;

/// Checksum after `FRAMES` frames of bot-vs-bot with seed 0xC0FFEE.
const GOLDEN_BOTS: u64 = 0x15fd_05bb_8237_0920;

/// A deterministic input stream with no randomness at all: two integer
/// sequences with different periods, so the pair never settles into a loop
/// shorter than the run.
fn scripted_inputs(frame: usize) -> [PlayerInput; 2] {
    let f = frame as u32;
    [
        PlayerInput((f.wrapping_mul(2_654_435_761) >> 13) as u16 & 0x03FF),
        PlayerInput((f.wrapping_mul(40_503).wrapping_add(7) >> 5) as u16 & 0x03FF),
    ]
}

fn replay_scripted() -> Arena {
    let mut arena = Arena::new();
    for frame in 0..FRAMES {
        arena.advance_frame(scripted_inputs(frame), OutputMode::Present);
    }
    arena
}

fn replay_bots(seed: u64) -> Arena {
    let mut arena = Arena::new();
    let mut p1 = ArenaBot::new(0, seed);
    let mut p2 = ArenaBot::new(1, seed);
    for _ in 0..FRAMES {
        let inputs = [p1.decide(&arena), p2.decide(&arena)];
        arena.advance_frame(inputs, OutputMode::Present);
    }
    arena
}

#[test]
fn a_hundred_thousand_frames_replay_to_the_same_checksum() {
    let a = replay_scripted();
    let b = replay_scripted();
    assert_eq!(
        a.checksum(),
        b.checksum(),
        "the arena disagreed with itself"
    );
    assert_eq!(
        a.save_state(),
        b.save_state(),
        "the checksum matched but the state did not"
    );

    assert_eq!(
        a.checksum(),
        GOLDEN_SCRIPTED,
        "simulation behaviour changed; if that was intended, update GOLDEN_SCRIPTED"
    );
}

#[test]
fn a_hundred_thousand_bot_frames_replay_to_the_same_checksum() {
    let a = replay_bots(0xC0FFEE);
    let b = replay_bots(0xC0FFEE);
    assert_eq!(a.checksum(), b.checksum());
    assert!(
        a.rounds_won[0] + a.rounds_won[1] > 10,
        "a 28-minute match should finish many rounds, got {:?}",
        a.rounds_won
    );

    assert_eq!(
        a.checksum(),
        GOLDEN_BOTS,
        "bot or simulation behaviour changed; update GOLDEN_BOTS if intended"
    );
}

#[test]
fn saving_and_restoring_mid_replay_changes_nothing() {
    // Rollback's core assumption, at scale: a state saved at frame N and
    // restored later must continue exactly as if it had never been touched.
    let mut straight = Arena::new();
    let mut interrupted = Arena::new();

    for frame in 0..FRAMES {
        let inputs = scripted_inputs(frame);
        straight.advance_frame(inputs, OutputMode::Present);

        // Save and restore every 997 frames -- a prime, so the interruption
        // lands on every phase of the simulation's own cycles.
        if frame % 997 == 0 {
            let blob = interrupted.save_state();
            interrupted.load_state(&blob).unwrap();
        }
        interrupted.advance_frame(inputs, OutputMode::Present);
    }

    assert_eq!(straight.checksum(), interrupted.checksum());
    assert_eq!(straight.save_state(), interrupted.save_state());
}

#[test]
fn health_and_positions_stay_inside_their_bounds_for_the_whole_replay() {
    let mut arena = Arena::new();
    for frame in 0..FRAMES {
        arena.advance_frame(scripted_inputs(frame), OutputMode::Present);
        for f in &arena.fighters {
            assert!(
                (0..=rollback_arena::MAX_HEALTH).contains(&f.health),
                "health {} out of range at frame {}",
                f.health,
                frame
            );
            assert!(
                (rollback_arena::STAGE_MIN_X..=rollback_arena::STAGE_MAX_X).contains(&f.x),
                "x {} out of range at frame {}",
                f.x,
                frame
            );
            assert!(f.y >= 0, "y {} below the floor at frame {}", f.y, frame);
        }
    }
}

#[test]
#[ignore = "prints the golden constants; run with --ignored to regenerate"]
fn print_golden_constants() {
    println!("GOLDEN_SCRIPTED = {:#018x};", replay_scripted().checksum());
    println!("GOLDEN_BOTS = {:#018x};", replay_bots(0xC0FFEE).checksum());
}
