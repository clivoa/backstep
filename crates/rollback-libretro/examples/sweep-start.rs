//! Find the frame on which pressing Start actually begins a match.
//!
//! `probe-boot` shows what the machine is drawing, which is enough to read most
//! boot timings off a contact sheet. It is not enough for the one timing that
//! matters most: the title screen accepts Start for a window of a few dozen
//! frames, and eyeballing a montage to locate that window is both imprecise and
//! easy to get wrong (frames the core does not draw are skipped, so tile N is
//! not necessarily frame N * step).
//!
//! So ask the machine instead. For each candidate frame, reset, insert coins,
//! press Start at the candidate, run on for a while, and dump the resulting
//! screen. One tile per candidate, labelled by candidate. The ones that entered
//! the game look nothing like the ones that stayed in the attract loop.
//!
//! ```text
//! cargo run --release -p rollback-libretro --example sweep-start -- \
//!     cores/fbneo_libretro.so lastbld2.zip artifacts/system out/ 690 800 10
//! ```
//!
//! This only means anything on a deterministic core -- otherwise each candidate
//! is a different machine and the sweep measures noise. Check with
//! `just check-determinism` first.

use std::io::Write;
use std::path::PathBuf;

use rollback_core::{Button, OutputMode, PlayerInput, Simulation};
use rollback_libretro::{host, LibretroCore, LibretroSimulation};

/// Mirrors the coin phase of `script::last_blade_2_boot`.
const BOOT_WAIT: u32 = 600;
const COIN_HOLD: u32 = 12;
/// Frames to hold Start. Overridable, because "the press is too short" is one
/// of the hypotheses this tool exists to kill.
fn start_hold() -> u32 {
    std::env::var("SWEEP_START_HOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12)
}
const COIN_GAP: u32 = 45;
/// Frames to run past the Start press before looking at the screen. Long enough
/// to be through the character-select intro if the press worked.
const SETTLE_FRAMES: u32 = 400;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: sweep-start <core.so> <rom> <system-dir> <out-dir> [from] [to] [step]");
        std::process::exit(2);
    }
    let core_path = PathBuf::from(&args[1]);
    let rom_path = PathBuf::from(&args[2]);
    let system_dir = args[3].clone();
    let out_dir = PathBuf::from(&args[4]);
    let from: u32 = args.get(5).map_or(690, |s| s.parse().unwrap());
    let to: u32 = args.get(6).map_or(800, |s| s.parse().unwrap());
    let step: u32 = args.get(7).map_or(10, |s| s.parse().unwrap());

    std::fs::create_dir_all(&system_dir).expect("system dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    host::set_directories(&system_dir, &system_dir);
    host::set_options(rollback_libretro::PINNED_CORE_OPTIONS);

    let mut core = LibretroCore::load(&core_path).expect("core must load");
    core.load_game(&rom_path).expect("rom must load");
    let mut sim = LibretroSimulation::new(core)
        .with_checksum_skip(rollback_libretro::core::CHECKSUM_SKIP_BYTES);

    // Which logical button to test. Sweeping this instead of the frame
    // answers a different question: not "when does Start work" but "does the
    // core hear Start at all", which is the right question once every frame in
    // four attract cycles has been ruled out.
    let button = std::env::var("SWEEP_BUTTON").ok();
    let buttons: Vec<Button> = match &button {
        Some(name) => vec![parse_button(name)],
        None => vec![Button::Start],
    };
    let sweep_buttons = std::env::var("SWEEP_ALL_BUTTONS").is_ok();
    let buttons = if sweep_buttons {
        Button::ALL.to_vec()
    } else {
        buttons
    };

    println!("sweeping {buttons:?} over frames {from}..={to} step {step}");
    for button in buttons {
        for candidate in (from..=to).step_by(step as usize) {
            // A fresh machine per candidate. This is only equivalent to a cold boot
            // because the core is deterministic; on an unpatched FBNeo each reset
            // would pick up a new clock reading and the sweep would be meaningless.
            sim.reset_machine();

            let total = candidate + start_hold() + SETTLE_FRAMES;
            for frame in 0..total {
                let input = script_input(frame, candidate, button);
                sim.advance_frame([input, input], OutputMode::Present);
            }

            let video = sim.video();
            let path = out_dir.join(format!("{button:?}-f{candidate:05}.ppm"));
            write_ppm(&path, &video);
            println!("  {button:?} at f{candidate:05} -> {}", path.display());
        }
    }
}

fn parse_button(name: &str) -> Button {
    Button::ALL
        .into_iter()
        .find(|b| format!("{b:?}").eq_ignore_ascii_case(name))
        .unwrap_or_else(|| panic!("unknown button '{name}'"))
}

/// The coin phase, then Start held for `COIN_HOLD` frames at `start_at`.
fn script_input(frame: u32, start_at: u32, button: Button) -> PlayerInput {
    let coin1 = BOOT_WAIT;
    let coin2 = BOOT_WAIT + COIN_HOLD + COIN_GAP;
    if (frame >= coin1 && frame < coin1 + COIN_HOLD)
        || (frame >= coin2 && frame < coin2 + COIN_HOLD)
    {
        return PlayerInput::NEUTRAL.with(Button::Coin);
    }
    if frame >= start_at && frame < start_at + start_hold() {
        return PlayerInput::NEUTRAL.with(button);
    }
    PlayerInput::NEUTRAL
}

fn write_ppm(path: &std::path::Path, video: &host::VideoFrame) {
    let mut out = Vec::with_capacity(video.pixels.len() * 3 + 32);
    write!(out, "P6\n{} {}\n255\n", video.width, video.height).unwrap();
    for px in &video.pixels {
        out.push((px >> 16) as u8);
        out.push((px >> 8) as u8);
        out.push(*px as u8);
    }
    std::fs::write(path, out).expect("write frame");
}
