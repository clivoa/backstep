//! Load a libretro core and a ROM and report what the core says about them.
//!
//! Exists because a core that cannot load a ROM is otherwise silent: FBNeo
//! returns success from `retro_load_game` even when the romset is unusable, and
//! the only sign is that `retro_serialize_size` comes back zero. This prints
//! everything the frontend can see, so the failure has a shape.
//!
//! ```text
//! cargo run -p rollback-libretro --example inspect-core -- \
//!     cores/fbneo_libretro.so /path/to/sfa3.zip artifacts/system
//! ```

use std::path::PathBuf;

use rollback_libretro::{host, LibretroCore};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: inspect-core <core.so> <rom> [system-dir]");
        std::process::exit(2);
    }
    let core_path = PathBuf::from(&args[1]);
    let rom_path = PathBuf::from(&args[2]);
    let system_dir = args.get(3).cloned().unwrap_or_else(|| ".".to_string());

    std::fs::create_dir_all(&system_dir).ok();
    host::set_directories(&system_dir, &system_dir);
    host::set_options(rollback_libretro::PINNED_CORE_OPTIONS);

    println!("core        {}", core_path.display());
    println!("rom         {}", rom_path.display());
    println!("system dir  {system_dir}");

    let mut core = match LibretroCore::load(&core_path) {
        Ok(core) => core,
        Err(e) => {
            eprintln!("load failed: {e}");
            std::process::exit(1);
        }
    };
    println!("library     {} {}", core.library_name, core.library_version);
    println!("fullpath    {}", core.needs_fullpath());

    let result = core.load_game(&rom_path);

    println!("\n-- messages from the core --");
    let messages = host::messages();
    if messages.is_empty() {
        println!("(none)");
    }
    for m in &messages {
        println!("  {m}");
    }

    println!("\n-- core log --");
    let log = host::log_lines();
    if log.is_empty() {
        println!("(none -- the core did not ask for the log interface)");
    }
    for (level, line) in &log {
        println!("  [{}] {line}", host::log_level_name(*level));
    }

    println!("\n-- environment commands this host refused --");
    let refused = host::with_host(|h| h.unhandled_environment.clone());
    if refused.is_empty() {
        println!("(none)");
    }
    for cmd in &refused {
        println!("  {cmd} (0x{cmd:x})");
    }

    match result {
        Ok(()) => {
            let geometry = core.geometry();
            let timing = core.av_timing();
            println!("\n-- loaded --");
            println!("state size  {} bytes", core.state_size());
            println!(
                "geometry    {}x{} (max {}x{})",
                geometry.base_width, geometry.base_height, geometry.max_width, geometry.max_height
            );
            println!(
                "timing      {:.4} Hz, {} Hz audio",
                timing.fps, timing.sample_rate
            );
        }
        Err(e) => {
            println!("\n-- load failed --");
            println!("{e}");
            std::process::exit(1);
        }
    }
}
