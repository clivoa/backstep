//! Run a core forward and dump frames, to calibrate a boot macro by looking.
//!
//! The boot scripts in `script.rs` are purely time-based -- hold a button for N
//! frames, wait M, move on -- because reading the game's RAM would tie the lab
//! to ROM-revision-specific memory offsets (the reasoning is in that module).
//! The cost of that choice is that the frame numbers have to come from
//! somewhere, and guessing them produces a script that presses Start during a
//! logo and silently ends up on the wrong screen.
//!
//! So: run the machine, dump what it draws, and read the numbers off the
//! screenshots.
//!
//! ```text
//! # free-run, no input, one PNG every 60 frames for 20 seconds
//! cargo run --release -p rollback-libretro --example probe-boot -- \
//!     cores/fbneo_libretro.so rom.zip artifacts/system out/ 1200 60
//!
//! # the same, but running the game's real boot script
//! PROBE_SCRIPT=lastblade2 cargo run --release ... out/ 3000 30
//! ```
//!
//! Frames are written as binary PPM, which needs no image crate; turn them into
//! something viewable with ImageMagick:
//!
//! ```text
//! magick montage out/*.ppm -tile 6x -geometry +2+2 out/contact-sheet.png
//! ```

use std::io::Write;
use std::path::PathBuf;

use rollback_core::{OutputMode, PlayerInput, Simulation};
use rollback_libretro::script::{Game, ScriptedBot};
use rollback_libretro::{host, LibretroCore, LibretroSimulation};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: probe-boot <core.so> <rom> <system-dir> <out-dir> [frames] [every]");
        eprintln!("       PROBE_SCRIPT=sfa3|lastblade2 runs that game's boot macro");
        std::process::exit(2);
    }
    let core_path = PathBuf::from(&args[1]);
    let rom_path = PathBuf::from(&args[2]);
    let system_dir = args[3].clone();
    let out_dir = PathBuf::from(&args[4]);
    let frames: u32 = args.get(5).map_or(1_200, |s| s.parse().unwrap_or(1_200));
    let every: u32 = args.get(6).map_or(60, |s| s.parse().unwrap_or(60));

    let script = std::env::var("PROBE_SCRIPT").ok().map(|name| {
        name.parse::<Game>()
            .unwrap_or_else(|e| panic!("PROBE_SCRIPT: {e}"))
    });

    std::fs::create_dir_all(&system_dir).expect("system dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    host::set_directories(&system_dir, &system_dir);
    host::set_options(rollback_libretro::PINNED_CORE_OPTIONS);

    let mut core = LibretroCore::load(&core_path).expect("core must load");
    core.load_game(&rom_path).expect("rom must load");
    println!(
        "{} {} | state {} bytes | {}x{} | {:.4} Hz",
        core.library_name,
        core.library_version,
        core.state_size(),
        core.geometry().base_width,
        core.geometry().base_height,
        core.av_timing().fps
    );
    core.reset();

    // Same director both peers would run, so the screenshots show exactly the
    // screens a real session would land on.
    let director = script.map(rollback_libretro::script::BootDirector::new);
    if let Some(d) = &director {
        println!(
            "script {:?}: hands over at frame {} ({:.1} s)",
            script.unwrap(),
            d.ready_at(),
            f64::from(d.ready_at()) / 60.0
        );
    }

    // Once the boot script hands over, both sides are played by the same
    // scripted bots a real session uses -- otherwise the probe would show a
    // match where nobody moves, which is not the thing worth looking at.
    let seed: u64 = std::env::var("PROBE_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4242);
    let mut bots = script.map(|g| [ScriptedBot::new(g, seed, 0), ScriptedBot::new(g, seed, 1)]);

    let mut sim = LibretroSimulation::new(core)
        .with_checksum_skip(rollback_libretro::core::CHECKSUM_SKIP_BYTES);
    let mut written = 0;
    for frame in 0..frames {
        let inputs = match (&director, &mut bots) {
            (Some(d), Some(b)) => [
                d.input(frame, 0).unwrap_or_else(|| b[0].decide()),
                d.input(frame, 1).unwrap_or_else(|| b[1].decide()),
            ],
            _ => [PlayerInput::NEUTRAL; 2],
        };
        sim.advance_frame(inputs, OutputMode::Present);

        // Checksums at fixed frames: run the probe twice and compare these to
        // tell a mis-measured constant from a machine that is not reproducible.
        if frame > 0 && frame % 300 == 0 {
            println!("checksum f{frame:06} {:016x}", sim.checksum());
        }

        if frame % every == 0 {
            let video = sim.video();
            if video.is_empty() {
                continue;
            }
            let path = out_dir.join(format!("f{frame:06}.ppm"));
            write_ppm(&path, &video);
            written += 1;
        }
    }
    println!("wrote {written} frames to {}", out_dir.display());

    // Which RetroPad ids did the core actually read, and how often did it see
    // them held? An id with zero polls is not wired up at all; an id with polls
    // but zero presses means the script never pressed it.
    const NAMES: [&str; 16] = [
        "B", "Y", "SELECT", "START", "UP", "DOWN", "LEFT", "RIGHT", "A", "X", "L", "R", "L2", "R2",
        "L3", "R3",
    ];
    let (polled, pressed) = host::with_host(|h| (h.polled, h.polled_pressed));
    println!("\n-- RetroPad polling (port: polls/pressed) --");
    for id in 0..16 {
        if polled[0][id] == 0 && polled[1][id] == 0 {
            continue;
        }
        println!(
            "  {:<7} p1 {:>7}/{:<7} p2 {:>7}/{:<7}",
            NAMES[id], polled[0][id], pressed[0][id], polled[1][id], pressed[1][id]
        );
    }
}

/// Binary PPM: three bytes per pixel, no compression, no dependencies.
fn write_ppm(path: &std::path::Path, video: &host::VideoFrame) {
    let mut out = Vec::with_capacity(video.pixels.len() * 3 + 32);
    write!(out, "P6\n{} {}\n255\n", video.width, video.height).unwrap();
    for px in &video.pixels {
        // The host normalises everything to XRGB8888.
        out.push((px >> 16) as u8);
        out.push((px >> 8) as u8);
        out.push(*px as u8);
    }
    std::fs::write(path, out).expect("write frame");
}
