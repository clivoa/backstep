//! Exercises the real FFI path -- `dlopen`, callbacks, `retro_run`,
//! `retro_serialize`/`retro_unserialize` -- against `fake-libretro-core`.
//!
//! No ROM is involved, so CI can run this anywhere.
//!
//! Every test lives in this one file and shares a lock, because a libretro core
//! is a process-wide singleton: two tests loading a core at the same time is
//! precisely the situation `LibretroCore::load` refuses.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use rollback_core::{
    AdvanceOutcome, Button, OutputMode, PlayerHandle, PlayerInput, RollbackSession, SessionConfig,
    Simulation, SimulationKind,
};
use rollback_libretro::{LibretroCore, LibretroSimulation};

static CORE_LOCK: Mutex<()> = Mutex::new(());

fn serialised() -> MutexGuard<'static, ()> {
    CORE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Path to the `cdylib` cargo just built for `fake-libretro-core`.
fn fake_core_path() -> PathBuf {
    // The test binary lives in target/<profile>/deps/; the cdylib is one level up.
    let mut dir = std::env::current_exe().expect("test binary has a path");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    let candidates = [
        dir.join("libfake_libretro_core.so"),
        dir.join("deps").join("libfake_libretro_core.so"),
    ];
    candidates
        .iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| {
            panic!("fake core not found; looked in {candidates:?}");
        })
        .clone()
}

fn load() -> LibretroSimulation {
    let mut core = LibretroCore::load(fake_core_path()).expect("fake core must load");
    core.load_game(&PathBuf::from("/dev/null"))
        .expect("fake core accepts any game");
    LibretroSimulation::new(core)
}

fn press(buttons: &[Button]) -> PlayerInput {
    buttons
        .iter()
        .fold(PlayerInput::NEUTRAL, |acc, &b| acc.with(b))
}

#[test]
fn the_core_loads_reports_itself_and_unloads() {
    let _guard = serialised();
    let core = LibretroCore::load(fake_core_path()).unwrap();
    assert_eq!(core.library_name, "fake-libretro-core");
    assert_eq!(core.library_version, "0.1.0");
    assert!(!core.needs_fullpath());
    drop(core);

    // The singleton latch must be released, so a second load succeeds.
    let again = LibretroCore::load(fake_core_path()).unwrap();
    assert_eq!(again.library_name, "fake-libretro-core");
}

#[test]
fn the_core_can_log_through_the_c_variadic_shim() {
    let _guard = serialised();
    rollback_libretro::host::reset_state();
    let _core = LibretroCore::load(fake_core_path()).unwrap();

    // The fake core logs one ERROR line shaped like FBNeo's missing-ROM report.
    // Getting this back means the C shim ran `vsnprintf` over the varargs and
    // handed the finished string to Rust -- the whole point of the shim, and
    // the only channel on which FBNeo names a file it could not find.
    let lines = rollback_libretro::host::log_lines();
    assert_eq!(
        lines,
        vec![(
            3,
            "ROM at index 7 with name fake.key and CRC 0x5474a3c6 is required".to_string()
        )],
        "the shim must format %d, %s and %08x, and strip the trailing newline"
    );

    // And the error-only view is what a load failure would quote.
    assert_eq!(
        rollback_libretro::host::render_log_errors(),
        "\n  core error: ROM at index 7 with name fake.key and CRC 0x5474a3c6 is required"
    );
}

#[test]
fn a_second_core_in_the_same_process_is_refused() {
    let _guard = serialised();
    let first = LibretroCore::load(fake_core_path()).unwrap();
    let second = LibretroCore::load(fake_core_path());
    assert!(
        matches!(second, Err(rollback_libretro::CoreError::AlreadyLoaded)),
        "libretro cores keep state in globals; two at once must be refused"
    );
    drop(first);
}

#[test]
fn loading_a_game_reports_a_usable_state_size_and_geometry() {
    let _guard = serialised();
    let mut core = LibretroCore::load(fake_core_path()).unwrap();
    core.load_game(&PathBuf::from("/dev/null")).unwrap();
    assert_eq!(core.state_size(), fake_libretro_core::STATE_SIZE);
    assert_eq!(core.geometry().base_width, fake_libretro_core::WIDTH);
    assert_eq!(core.av_timing().fps, 60.0);
}

#[test]
fn advancing_produces_video_and_serialisable_state() {
    let _guard = serialised();
    let mut sim = load();

    let before = sim.save_state();
    sim.advance_frame(
        [press(&[Button::Right]), PlayerInput::NEUTRAL],
        OutputMode::Present,
    );
    let after = sim.save_state();

    assert_eq!(before.len(), fake_libretro_core::STATE_SIZE);
    assert_ne!(before, after, "a frame must change the machine state");

    let video = sim.video();
    assert_eq!(video.width, fake_libretro_core::WIDTH);
    assert_eq!(video.height, fake_libretro_core::HEIGHT);
    assert!(!video.is_empty());
    assert!(!sim.take_audio().is_empty(), "the core emitted audio");
}

#[test]
fn a_state_restores_exactly() {
    let _guard = serialised();
    let mut sim = load();
    for f in 0..60u16 {
        sim.advance_frame(
            [PlayerInput(f & 0xFF), PlayerInput((f * 3) & 0xFF)],
            OutputMode::Present,
        );
    }

    let snapshot = sim.save_state();
    let checksum = sim.checksum();

    for _ in 0..30 {
        sim.advance_frame(
            [press(&[Button::Left]), press(&[Button::Attack])],
            OutputMode::Present,
        );
    }
    assert_ne!(sim.checksum(), checksum);

    sim.load_state(&snapshot).unwrap();
    assert_eq!(sim.checksum(), checksum, "restore must be exact");
    assert_eq!(sim.save_state(), snapshot);
}

#[test]
fn a_checksum_skip_that_swallows_the_state_is_refused() {
    // The FBNeo skip is 2048 bytes; this core's whole state is 32. Applying it
    // blindly made `checksum` hash an empty slice, so every state hashed to the
    // same value and desync detection quietly became a no-op that could never
    // fire. A constant checksum is worse than no checksum, because it looks
    // like agreement.
    let _guard = serialised();
    let mut core = LibretroCore::load(fake_core_path()).unwrap();
    core.load_game(&PathBuf::from("/dev/null")).unwrap();
    let state_size = core.state_size();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        LibretroSimulation::new(core)
            .with_checksum_skip(rollback_libretro::core::CHECKSUM_SKIP_BYTES)
    }));
    assert!(
        result.is_err(),
        "a {}-byte skip must be refused on a {state_size}-byte state",
        rollback_libretro::core::CHECKSUM_SKIP_BYTES
    );
}

#[test]
fn a_modest_checksum_skip_still_distinguishes_states() {
    let _guard = serialised();
    let mut sim = load().with_checksum_skip(8);
    let before = sim.checksum();
    for _ in 0..20 {
        sim.advance_frame(
            [press(&[Button::Right]), press(&[Button::Left])],
            OutputMode::Present,
        );
    }
    assert_ne!(
        sim.checksum(),
        before,
        "skipping a prefix must not stop the checksum from noticing the rest"
    );
}

#[test]
fn a_wrong_sized_state_is_refused() {
    let _guard = serialised();
    let mut sim = load();
    let err = sim.load_state(&[0u8; 7]).unwrap_err();
    assert!(matches!(
        err,
        rollback_core::SimulationError::StateSize { actual: 7, .. }
    ));
}

#[test]
fn resimulated_frames_produce_no_video_or_audio() {
    let _guard = serialised();
    let mut sim = load();

    // Drain whatever the first presented frame produced.
    sim.advance_frame([PlayerInput::NEUTRAL; 2], OutputMode::Present);
    let _ = sim.take_audio();
    let presented = sim.video();

    for _ in 0..10 {
        sim.advance_frame(
            [press(&[Button::Right]), PlayerInput::NEUTRAL],
            OutputMode::Resimulate,
        );
    }

    assert!(
        sim.take_audio().is_empty(),
        "re-simulated audio must be discarded, not replayed at ten times speed"
    );
    let after = sim.video();
    assert_eq!(
        after.pixels, presented.pixels,
        "the displayed frame must not have been overwritten by a replayed one"
    );
    assert_eq!(sim.resimulated_frames, 10);
    assert_eq!(sim.presented_frames, 1);
}

#[test]
fn output_mode_does_not_change_the_machine_state() {
    let _guard = serialised();
    let script: Vec<[PlayerInput; 2]> = (0..120u16)
        .map(|f| [PlayerInput(f & 0x7F), PlayerInput((f * 5) & 0x7F)])
        .collect();

    let shown = {
        let mut sim = load();
        for inputs in &script {
            sim.advance_frame(*inputs, OutputMode::Present);
        }
        sim.save_state()
    };
    let replayed = {
        let mut sim = load();
        for inputs in &script {
            sim.advance_frame(*inputs, OutputMode::Resimulate);
        }
        sim.save_state()
    };
    assert_eq!(shown, replayed);
}

#[test]
fn a_full_rollback_session_over_the_ffi_converges() {
    let _guard = serialised();
    let config = SessionConfig {
        simulation: SimulationKind::LastBlade2,
        ..Default::default()
    };

    // Remote inputs known up front: the reference run, no prediction at all.
    let remote: Vec<PlayerInput> = (0..200u16).map(|f| PlayerInput((f % 13) & 0x7F)).collect();
    let local = press(&[Button::Right]);

    let clean_checksum = {
        let mut session = RollbackSession::new(load(), config, PlayerHandle::P1).unwrap();
        session.add_remote_inputs(1, &remote).unwrap();
        for _ in 0..200 {
            session.add_local_input(local).unwrap();
            assert!(matches!(
                session.advance().unwrap(),
                AdvanceOutcome::Advanced { .. }
            ));
        }
        session.simulation().checksum()
    };

    // The same inputs arriving three frames late, forcing prediction and
    // rollback through `retro_unserialize`.
    let mut session = RollbackSession::new(load(), config, PlayerHandle::P1).unwrap();
    let mut delivered: i32 = 0;
    for f in 0..200i32 {
        session.add_local_input(local).unwrap();
        assert!(matches!(
            session.advance().unwrap(),
            AdvanceOutcome::Advanced { .. }
        ));
        let target = f - 3;
        if target > delivered {
            session
                .add_remote_inputs(delivered + 1, &remote[delivered as usize..target as usize])
                .unwrap();
            delivered = target;
        }
    }
    session
        .add_remote_inputs(delivered + 1, &remote[delivered as usize..200])
        .unwrap();

    assert!(
        session.stats().rollbacks > 0,
        "the late run must have rolled back"
    );
    assert_eq!(
        session.simulation().checksum(),
        clean_checksum,
        "rollback through the libretro FFI must converge on the clean state"
    );
}
