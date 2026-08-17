//! The local peer: a human on P1, watching the rollback happen.
//!
//! Dials the EC2 instance on UDP/7000, runs the same session loop as the
//! headless bot, and draws the simulation plus an overlay that makes the
//! netcode visible. For SFA3 the boot script owns P1 until the match starts;
//! after that every frame comes from the controller.

mod font;
mod input;
mod overlay;
mod render;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Parser;
use rollback_arena::Arena;
use rollback_core::{NetworkProfile, PlayerHandle, SessionConfig, Simulation, SimulationKind};
use rollback_libretro::{LibretroCore, LibretroSimulation, Sfa3Director};
use rollback_net::UdpTransport;
use rollback_runner::{app, handshake, Role, RunnerConfig, SessionRunner, StepOutcome};
use rollback_telemetry::SessionInfo;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;

use overlay::{FrameMark, Overlay};
use render::Renderer;

#[derive(Parser, Debug)]
#[command(name = "rollback-client", about = "Play P1 against the remote peer.")]
struct Args {
    /// Which simulation to run. Must match the peer.
    #[arg(long, default_value = "arena")]
    sim: SimulationKind,

    /// Public address of the remote peer.
    #[arg(long)]
    peer: SocketAddr,

    /// Local address to bind. Port 0 lets the OS choose.
    #[arg(long, default_value_t = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0))]
    bind: SocketAddr,

    /// Player slot. The local human is P1.
    #[arg(long, default_value = "p1")]
    player: String,

    /// Named impairment profile applied to outgoing datagrams.
    #[arg(long, default_value = "natural")]
    profile: String,

    #[arg(long, default_value_t = SessionConfig::default().seed)]
    seed: u64,

    #[arg(long, default_value_t = SessionConfig::default().input_delay)]
    input_delay: u8,

    #[arg(long, default_value_t = SessionConfig::default().prediction_limit)]
    prediction_limit: u8,

    /// Seconds to play. 0 means until the window is closed.
    #[arg(long, default_value_t = 0)]
    duration: u64,

    #[arg(long)]
    core: Option<PathBuf>,

    #[arg(long)]
    rom: Option<PathBuf>,

    #[arg(long, default_value = "artifacts/system")]
    system_dir: PathBuf,

    #[arg(long, default_value = "artifacts/logs")]
    log_dir: PathBuf,

    #[arg(long, default_value = rollback_telemetry::DEFAULT_EXPORTER_ADDR)]
    metrics: String,

    #[arg(long)]
    no_metrics: bool,

    #[arg(long, default_value = "play")]
    mode: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let player = match args.player.to_lowercase().as_str() {
        "p1" | "1" => PlayerHandle::P1,
        "p2" | "2" => PlayerHandle::P2,
        other => bail!("unknown player '{other}' (expected p1|p2)"),
    };
    let (profile_name, profile) = NetworkProfile::named(&args.profile).with_context(|| {
        format!(
            "unknown profile '{}' (expected one of {})",
            args.profile,
            NetworkProfile::NAMES.join(", ")
        )
    })?;

    let config = SessionConfig {
        simulation: args.sim,
        seed: args.seed,
        input_delay: args.input_delay,
        prediction_limit: args.prediction_limit,
        network: profile,
        ..Default::default()
    };
    config.validate()?;

    let auth = app::session_key_from_env().map_err(anyhow::Error::msg)?;
    let core_hash = app::hash_or_absent(args.core.as_deref())?;
    let rom_hash = app::hash_or_absent(args.rom.as_deref())?;

    let mut transport = UdpTransport::bind(args.bind, auth, profile)
        .with_context(|| format!("binding {}", args.bind))?;
    transport.set_peer(args.peer);

    let local_identity = app::identity(&config, player, core_hash, rom_hash);
    eprintln!(
        "rollback-client {} | sim={} peer={} profile={profile_name}",
        &app::APP_COMMIT[..app::APP_COMMIT.len().min(12)],
        args.sim.as_str(),
        args.peer,
    );

    let remote = handshake(
        &mut transport,
        Role::Client,
        local_identity,
        Duration::from_secs(60),
    )
    .context("handshake failed")?;
    eprintln!(
        "connected: {}",
        rollback_runner::handshake::describe(&local_identity, &remote)
    );

    let mut info = SessionInfo::new(args.sim, profile_name, "p1");
    info.app_commit = app::APP_COMMIT.to_string();
    info.core_sha256 = app::digest_hex(&core_hash);
    info.rom_sha256 = app::digest_hex(&rom_hash);
    info.seed = config.seed;
    info.input_delay = config.input_delay;
    info.prediction_limit = config.prediction_limit;
    info.state_history = config.state_history;

    let runner_config = RunnerConfig {
        session: config,
        local_player: player,
        log_dir: args.log_dir.clone(),
        session_name: app::session_name(args.sim, profile_name, player, &args.mode),
        exporter_addr: (!args.no_metrics).then(|| args.metrics.clone()),
        info,
    };

    let frame_budget = if args.duration == 0 {
        u64::MAX
    } else {
        args.duration * u64::from(config.tick_rate_hz)
    };

    // --- SDL ---
    let sdl = sdl2::init().map_err(anyhow::Error::msg)?;
    let video = sdl.video().map_err(anyhow::Error::msg)?;
    let controllers = sdl.game_controller().map_err(anyhow::Error::msg)?;
    let window = video
        .window(
            &format!(
                "rollback-netcode :: {} :: {profile_name}",
                args.sim.as_str()
            ),
            render::WINDOW_W,
            render::WINDOW_H,
        )
        .position_centered()
        .build()?;
    // No vsync: the session, not the display, owns the frame clock. A 144 Hz
    // monitor must not make the simulation run at 144 Hz.
    let canvas = window.into_canvas().accelerated().build()?;
    let mut events = sdl.event_pump().map_err(anyhow::Error::msg)?;
    let mut renderer = Renderer::new(canvas);

    // Open the first gamepad, if any. A missing gamepad is not an error: the
    // keyboard is a complete input device on its own.
    let pad = (0..controllers.num_joysticks().unwrap_or(0))
        .find(|&i| controllers.is_game_controller(i))
        .and_then(|i| controllers.open(i).ok());
    if let Some(pad) = &pad {
        eprintln!("gamepad: {}", pad.name());
    }

    match args.sim {
        SimulationKind::Arena => {
            let runner = SessionRunner::new(Arena::new(), transport, runner_config)?;
            play(
                runner,
                frame_budget,
                &mut renderer,
                &mut events,
                pad.as_ref(),
                None,
                player.index(),
                |renderer, arena: &Arena, _| renderer.draw_arena(arena),
            )
        }
        SimulationKind::Sfa3 => {
            let core_path = args.core.context("--core is required for --sim sfa3")?;
            let rom_path = args.rom.context("--rom is required for --sim sfa3")?;
            std::fs::create_dir_all(&args.system_dir)?;
            let dir = args.system_dir.to_string_lossy().to_string();
            rollback_libretro::set_directories(&dir, &dir);
            rollback_libretro::set_options(rollback_libretro::PINNED_CORE_OPTIONS);

            let mut core = LibretroCore::load(&core_path)
                .with_context(|| format!("loading core {core_path:?}"))?;
            core.load_game(&rom_path)
                .with_context(|| format!("loading ROM {rom_path:?}"))?;
            let geometry = core.geometry();
            eprintln!(
                "core: {} {} | {}x{} | state {} bytes",
                core.library_name,
                core.library_version,
                geometry.base_width,
                geometry.base_height,
                core.state_size()
            );

            let creator = renderer.texture_creator();
            let mut texture = Renderer::video_texture(
                &creator,
                geometry.max_width.max(geometry.base_width),
                geometry.max_height.max(geometry.base_height),
            )?;

            let runner =
                SessionRunner::new(LibretroSimulation::new(core), transport, runner_config)?;
            play(
                runner,
                frame_budget,
                &mut renderer,
                &mut events,
                pad.as_ref(),
                Some(Sfa3Director::new()),
                player.index(),
                move |renderer, sim: &LibretroSimulation, _| {
                    renderer.draw_video(&mut texture, &sim.video())
                },
            )
        }
    }
}

/// The frame loop: input, session step, draw.
#[allow(clippy::too_many_arguments)]
fn play<S, D>(
    mut runner: SessionRunner<S>,
    frame_budget: u64,
    renderer: &mut Renderer,
    events: &mut sdl2::EventPump,
    pad: Option<&sdl2::controller::GameController>,
    director: Option<Sfa3Director>,
    slot: usize,
    mut draw: D,
) -> Result<()>
where
    S: Simulation,
    D: FnMut(&mut Renderer, &S, u32) -> Result<()>,
{
    let mut overlay = Overlay::new();
    let mut frames = 0u64;

    'outer: loop {
        for event in events.poll_iter() {
            match event {
                Event::Quit { .. }
                | Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => break 'outer,
                _ => {}
            }
        }

        let frame = runner.session().current_frame().max(0) as u32;

        // During the SFA3 boot the script owns the pad, so a stray press cannot
        // change the character selection on this peer only.
        let scripted = director.as_ref().and_then(|d| d.input(frame, slot));
        let local_input = scripted.unwrap_or_else(|| {
            let keyboard = input::from_keyboard(&events.keyboard_state());
            match pad {
                Some(pad) => input::combine([keyboard, input::from_controller(pad)]),
                None => input::combine([keyboard]),
            }
        });

        let mark = match runner.step(local_input)? {
            StepOutcome::Advanced { predicted, .. } => {
                frames += 1;
                if predicted {
                    FrameMark::Predicted
                } else {
                    FrameMark::Confirmed
                }
            }
            StepOutcome::Stalled { .. } => FrameMark::Stalled,
            StepOutcome::Ended(reason) => {
                eprintln!("session ended: {reason:?}");
                break 'outer;
            }
        };
        overlay.push(mark, runner.snapshot());

        renderer.begin();
        draw(renderer, runner.simulation(), frame)?;
        renderer.draw_overlay(&overlay, runner.snapshot())?;
        renderer.present();

        if frames >= frame_budget {
            eprintln!("frame budget reached ({frames} frames)");
            break;
        }
        runner.pace();
    }

    let desync = runner.snapshot().desync;
    if let Some(path) = runner.finish(if desync { "desync" } else { "normal" })? {
        eprintln!("log: {}", path.display());
    }
    if desync {
        bail!("session ended in a confirmed desync");
    }
    Ok(())
}
