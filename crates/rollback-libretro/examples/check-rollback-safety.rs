//! Does this core survive a rollback? Save, run on, restore, replay, compare.
//!
//! `check-determinism.sh` asks whether two *processes* agree. This asks a
//! different and equally load-bearing question of a single process: whether
//! `retro_unserialize` restores *everything* that `retro_run` will go on to
//! read.
//!
//! It has to be asked separately, because a core can be perfectly reproducible
//! from a cold boot and still keep some state outside its savestate -- battery
//! RAM, a coin counter, an audio buffer. Nothing notices until a rollback
//! crosses the moment that state changes, at which point the two peers diverge
//! and it looks like a bug in the rollback.
//!
//! The check, at each probe point:
//!
//! ```text
//!   run to frame N, snapshot S
//!   run K more frames with inputs I     -> checksum A
//!   restore S, run the same K frames    -> checksum B
//!   A == B ?
//! ```
//!
//! A is exactly what the peer that never rolled back computed; B is what the
//! peer that did rolled back computes. If they differ, the session desyncs.
//!
//! ```text
//! cargo run --release -p rollback-libretro --example check-rollback-safety -- \
//!     cores/fbneo_libretro.so lastbld2.zip artifacts/system lastblade2 0 2000 20
//! ```

use std::path::PathBuf;

use rollback_core::{OutputMode, PlayerInput, Simulation};
use rollback_libretro::script::{BootDirector, Game};
use rollback_libretro::{host, LibretroCore, LibretroSimulation};

/// How far to replay past each probe point. Deeper than the prediction limit
/// the lab uses (8), so a failure here is strictly worse than anything a real
/// session would attempt.
fn replay_depth() -> u32 {
    std::env::var("REPLAY_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12)
}

/// Contiguous byte ranges where two states of equal length disagree.
fn differing_runs(a: &[u8], b: &[u8]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut start = None;
    for i in 0..a.len().min(b.len()) {
        match (a[i] == b[i], start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                runs.push((s, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        runs.push((s, a.len()));
    }
    runs
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!(
            "usage: check-rollback-safety <core.so> <rom> <system-dir> <game> [from] [to] [step]"
        );
        std::process::exit(2);
    }
    let core_path = PathBuf::from(&args[1]);
    let rom_path = PathBuf::from(&args[2]);
    let system_dir = args[3].clone();
    let game: Game = args[4].parse().expect("game");
    let from: u32 = args.get(5).map_or(0, |s| s.parse().unwrap());
    let to: u32 = args.get(6).map_or(2_000, |s| s.parse().unwrap());
    let step: u32 = args.get(7).map_or(20, |s| s.parse().unwrap());

    std::fs::create_dir_all(&system_dir).expect("system dir");
    rollback_libretro::clear_persistent_state(std::path::Path::new(&system_dir), game.romset())
        .expect("clear nvram");
    host::set_directories(&system_dir, &system_dir);
    host::set_options(rollback_libretro::PINNED_CORE_OPTIONS);

    let mut core = LibretroCore::load(&core_path).expect("core must load");
    core.load_game(&rom_path).expect("rom must load");
    println!("state {} bytes | game {game}", core.state_size());
    core.reset();

    let director = BootDirector::new(game);
    let inputs_at = |frame: u32| -> [PlayerInput; 2] {
        [
            director.input(frame, 0).unwrap_or(PlayerInput::NEUTRAL),
            director.input(frame, 1).unwrap_or(PlayerInput::NEUTRAL),
        ]
    };

    // Deliberately *without* a checksum skip: this tool exists to discover
    // where the skip belongs, so it has to see the whole state.
    let mut sim = LibretroSimulation::new(core);
    let mut frame = 0u32;
    let mut failures: Vec<u32> = Vec::new();
    let mut round_trip_failures = 0;
    let mut probes = 0;
    // The number that decides whether the lab's checksum is trustworthy: how
    // far into the state does rollback instability ever reach?
    let mut highest_unstable = 0usize;

    while frame < to {
        // Advance to the next probe point.
        let target = frame.max(from);
        while frame < target {
            sim.advance_frame(inputs_at(frame), OutputMode::Present);
            frame += 1;
        }

        // Snapshot here, the way a session does every frame.
        let snapshot = sim.save_state();

        // Level one: does restoring a state get the machine back to where it
        // was at all? If this fails, the replay result below means nothing --
        // the core simply cannot round-trip its own savestate.
        sim.load_state(&snapshot).expect("restore");
        let restored = sim.save_state();
        if restored != snapshot {
            round_trip_failures += 1;
            // Which bytes did not survive? A small, fixed region is a piece of
            // state the core rebuilds on load; a large scattered one means the
            // savestate is simply not a faithful description of the machine.
            let runs = differing_runs(&snapshot, &restored);
            let total: usize = runs.iter().map(|(a, b)| b - a).sum();
            highest_unstable = highest_unstable.max(runs.last().map_or(0, |r| r.1));
            println!(
                "  f{frame:06}  ROUND-TRIP  {total} of {} bytes differ in {} run(s): {:?}",
                snapshot.len(),
                runs.len(),
                &runs[..runs.len().min(8)]
            );
        }

        // Straight line: what a peer that never rolled back computes.
        let mut straight = frame;
        for _ in 0..replay_depth() {
            sim.advance_frame(inputs_at(straight), OutputMode::Present);
            straight += 1;
        }
        let expected_blob = sim.save_state();
        let expected = sim.checksum();

        // Rolled back: restore, replay the identical inputs.
        sim.load_state(&snapshot).expect("restore");
        let mut replay = frame;
        for _ in 0..replay_depth() {
            sim.advance_frame(inputs_at(replay), OutputMode::Resimulate);
            replay += 1;
        }
        let actual_blob = sim.save_state();
        let actual = sim.checksum();

        probes += 1;
        if expected != actual {
            failures.push(frame);
            let runs = differing_runs(&expected_blob, &actual_blob);
            let total: usize = runs.iter().map(|(a, b)| b - a).sum();
            highest_unstable = highest_unstable.max(runs.last().map_or(0, |r| r.1));
            println!(
                "  f{frame:06}  after {} replayed frames: {total} bytes differ in {} run(s), \
                 highest offset {}: {:?}",
                replay_depth(),
                runs.len(),
                runs.last().map_or(0, |r| r.1),
                &runs[..runs.len().min(6)]
            );
        }

        frame = straight.max(frame + step);
        // Get back onto the straight-line timeline before the next probe.
        while replay < frame {
            sim.advance_frame(inputs_at(replay), OutputMode::Present);
            replay += 1;
        }
    }

    println!(
        "\n{probes} probes, {round_trip_failures} save/load round-trip failures, \
         {} replay mismatches",
        failures.len()
    );
    // A mismatch is expected and harmless *if* it stays inside the region the
    // checksum already ignores. What would be fatal is instability reaching the
    // game's own memory, because then a rollback really does change the match.
    let skip = rollback_libretro::core::CHECKSUM_SKIP_BYTES;
    println!("highest unstable offset: {highest_unstable} (checksum ignores the first {skip})");

    if highest_unstable >= skip {
        println!(
            "\nNOT rollback-safe: instability reached offset {highest_unstable}, at or past \
             the {skip}-byte boundary the checksum ignores. Either the boundary is wrong or \
             this core cannot be rolled back. Do not trust a desync verdict until this passes."
        );
        std::process::exit(1);
    }

    println!(
        "\nROLLBACK-SAFE over frames {from}..{to}: every difference a rollback introduces \
         stays inside the ignored prefix; the machine state past it is reproduced exactly."
    );
}
