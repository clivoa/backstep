//! The headless peer.
//!
//! Normally P2 on the Frankfurt EC2 instance: it binds UDP/7000, waits for the
//! local client to dial it, and plays with a deterministic FSM. It is also both
//! ends of `just bench`, where two of these play each other so a run is exactly
//! repeatable from its seed.
//!
//! There is no display, no audio device and no window here -- the same session
//! loop as the client, with the controller replaced by a state machine.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Parser;
use rollback_arena::{Arena, ArenaBot};
use rollback_core::{
    NetworkProfile, PlayerHandle, PlayerInput, SessionConfig, Simulation, SimulationKind,
};
use rollback_libretro::{BootDirector, Game, LibretroCore, LibretroSimulation, ScriptedBot};
use rollback_net::{Authenticator, UdpTransport, DEFAULT_PORT};
use rollback_runner::{
    app, handshake, Role, RunnerConfig, SessionRunner, StepOutcome, VideoRecorder,
};
use rollback_telemetry::SessionInfo;

#[derive(Parser, Debug)]
#[command(
    name = "rollback-bot",
    about = "Headless rollback peer driven by a deterministic FSM."
)]
struct Args {
    /// Which simulation to run.
    #[arg(long, default_value = "arena")]
    sim: SimulationKind,

    /// Player slot to take. The EC2 side is P2.
    #[arg(long, default_value = "p2")]
    player: String,

    /// Address to bind. The lab uses UDP/7000 on all interfaces.
    #[arg(long, default_value_t = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_PORT))]
    bind: SocketAddr,

    /// Peer to dial. Omit to wait for the peer to dial us (the normal case).
    #[arg(long)]
    peer: Option<SocketAddr>,

    /// Named impairment profile applied to outgoing datagrams.
    #[arg(long, default_value = "natural")]
    profile: String,

    /// Session seed. Must match the peer.
    #[arg(long, default_value_t = SessionConfig::default().seed)]
    seed: u64,

    /// Frames of local input delay. Must match the peer.
    #[arg(long, default_value_t = SessionConfig::default().input_delay)]
    input_delay: u8,

    /// How far ahead of the peer to speculate. Must match the peer.
    #[arg(long, default_value_t = SessionConfig::default().prediction_limit)]
    prediction_limit: u8,

    /// How many saved states to keep. Must exceed `--prediction-limit`, and
    /// must match the peer.
    ///
    /// Worth raising with the prediction limit on a long link: a rollback can
    /// reach back `prediction_limit` frames, so the state at that frame has to
    /// still be in the buffer.
    #[arg(long, default_value_t = SessionConfig::default().state_history)]
    state_history: u8,

    /// Seconds to play before disconnecting cleanly. 0 means run until stopped.
    #[arg(long, default_value_t = 180)]
    duration: u64,

    /// libretro core, required for any emulated simulation.
    #[arg(long)]
    core: Option<PathBuf>,

    /// ROM, required for any emulated simulation. Never distributed with this
    /// repo.
    #[arg(long)]
    rom: Option<PathBuf>,

    /// Directory FBNeo reads NVRAM and settings from. Must be identical on
    /// both peers -- see docs/05-determinism.md.
    #[arg(long, default_value = "artifacts/system")]
    system_dir: PathBuf,

    /// Record the presented frames to this MP4. Needs ffmpeg on PATH, and an
    /// emulated simulation -- the arena has no framebuffer.
    ///
    /// What lands in the file is exactly the frames the player would have seen:
    /// one per advanced frame, none of the re-simulated ones.
    #[arg(long)]
    record: Option<PathBuf>,

    /// Where to write the JSONL session log.
    #[arg(long, default_value = "artifacts/logs")]
    log_dir: PathBuf,

    /// Prometheus exporter address. Loopback only, by design.
    #[arg(long, default_value = rollback_telemetry::DEFAULT_EXPORTER_ADDR)]
    metrics: String,

    /// Disable the Prometheus exporter.
    #[arg(long)]
    no_metrics: bool,

    /// Label recorded in the log and the metrics, e.g. `play` or `bench`.
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
        state_history: args.state_history,
        network: profile,
        ..Default::default()
    };
    config.validate()?;

    let auth = Authenticator::from_hex(
        &std::env::var("ROLLBACK_SESSION_KEY")
            .map_err(|_| anyhow::anyhow!("set ROLLBACK_SESSION_KEY (64 hex characters)"))?,
    )
    .map_err(|e| anyhow::anyhow!("invalid ROLLBACK_SESSION_KEY: {e}"))?;

    let core_hash = app::hash_or_absent(args.core.as_deref())
        .with_context(|| format!("hashing core {:?}", args.core))?;
    let bios = app::bios_path(args.sim, &args.system_dir);
    if let Some(bios) = &bios {
        anyhow::ensure!(
            bios.exists(),
            "{} needs the Neo Geo BIOS at {}. Both peers must use the same file: \
             it is hashed into the handshake, and it is half the code that boots the game.",
            args.sim.as_str(),
            bios.display()
        );
    }
    let rom_hash = app::hash_rom_set(args.rom.as_deref(), bios.as_deref())
        .with_context(|| format!("hashing ROM {:?}", args.rom))?;

    let mut transport = UdpTransport::bind(args.bind, auth, profile)
        .with_context(|| format!("binding {}", args.bind))?;
    if let Some(peer) = args.peer {
        transport.set_peer(peer);
    }

    let role = if args.peer.is_some() {
        Role::Client
    } else {
        Role::Host
    };
    let local_identity = app::identity(&config, player, core_hash, rom_hash);

    eprintln!(
        "rollback-bot {} | sim={} player={player:?} profile={profile_name} bind={} role={role:?}",
        &app::APP_COMMIT[..app::APP_COMMIT.len().min(12)],
        args.sim.as_str(),
        args.bind,
    );
    eprintln!("waiting for the peer (handshake timeout 120 s)...");

    let remote = handshake(
        &mut transport,
        role,
        local_identity,
        Duration::from_secs(120),
    )
    .context("handshake failed")?;
    eprintln!(
        "connected: {}",
        rollback_runner::handshake::describe(&local_identity, &remote)
    );

    let mut info = SessionInfo::new(
        args.sim,
        profile_name,
        if player == PlayerHandle::P1 {
            "p1"
        } else {
            "p2"
        },
    );
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

    match args.sim {
        SimulationKind::Arena => {
            let mut bot = ArenaBot::new(player.index(), config.seed);
            let runner = SessionRunner::new(Arena::new(), transport, runner_config)?;
            session_loop(runner, frame_budget, move |arena: &Arena, _frame| {
                bot.decide(arena)
            })
        }
        SimulationKind::Sfa3 | SimulationKind::LastBlade2 => {
            let game = match args.sim {
                SimulationKind::LastBlade2 => Game::LastBlade2,
                _ => Game::Sfa3,
            };
            let core_path = args
                .core
                .with_context(|| format!("--core is required for --sim {game}"))?;
            let rom_path = args
                .rom
                .with_context(|| format!("--rom is required for --sim {game}"))?;
            std::fs::create_dir_all(&args.system_dir)?;
            rollback_libretro::set_directories(
                &args.system_dir.to_string_lossy(),
                &args.system_dir.to_string_lossy(),
            );
            rollback_libretro::set_options(rollback_libretro::PINNED_CORE_OPTIONS);

            // Both peers must boot a machine in the same state, and FBNeo
            // persists one between runs. See `clear_persistent_state`.
            let cleared =
                rollback_libretro::clear_persistent_state(&args.system_dir, game.romset())
                    .with_context(|| format!("clearing saved state in {:?}", args.system_dir))?;
            for path in &cleared {
                eprintln!("cleared stale machine state: {}", path.display());
            }

            let mut core = LibretroCore::load(&core_path)
                .with_context(|| format!("loading core {core_path:?}"))?;
            core.load_game(&rom_path)
                .with_context(|| format!("loading ROM {rom_path:?}"))?;
            eprintln!(
                "core: {} {} | {} | state {} bytes | {:.2} Hz native",
                core.library_name,
                core.library_version,
                game,
                core.state_size(),
                core.av_timing().fps
            );

            let geometry = core.geometry();
            let mut recorder = match &args.record {
                Some(path) => {
                    let rec = VideoRecorder::start(
                        path,
                        geometry.base_width,
                        geometry.base_height,
                        config.tick_rate_hz,
                    )?;
                    eprintln!(
                        "recording {}x{} at {} Hz to {}",
                        geometry.base_width,
                        geometry.base_height,
                        config.tick_rate_hz,
                        path.display()
                    );
                    Some(rec)
                }
                None => None,
            };

            let runner = SessionRunner::new(
                LibretroSimulation::new(core)
                    .with_checksum_skip(rollback_libretro::core::CHECKSUM_SKIP_BYTES),
                transport,
                runner_config,
            )?;
            let director = BootDirector::new(game);
            let mut bot = ScriptedBot::new(game, config.seed, player.index());
            let slot = player.index();
            let result = session_loop_observed(
                runner,
                frame_budget,
                move |_sim: &LibretroSimulation, frame| {
                    // The boot script owns both players until the match starts, so
                    // a stray input cannot change the character selection on one
                    // peer only.
                    match director.input(frame, slot) {
                        Some(scripted) => scripted,
                        None => bot.decide(),
                    }
                },
                |sim: &LibretroSimulation| {
                    if let Some(rec) = recorder.as_mut() {
                        let frame = sim.video();
                        rec.push(frame.width, frame.height, &frame.pixels)?;
                    }
                    Ok(())
                },
            );

            // Close the file even if the session ended badly: a recording of a
            // session that desynced is exactly the one worth watching.
            if let Some(rec) = recorder.take() {
                let written = rec.frames_written;
                let skipped = rec.frames_skipped;
                match rec.finish() {
                    Ok(path) => eprintln!(
                        "recorded {written} frames to {} ({skipped} skipped)",
                        path.display()
                    ),
                    Err(e) => eprintln!("recording failed to finish: {e}"),
                }
            }
            result
        }
    }
}

/// The main loop, shared by both simulations.
///
/// `next_input` gets the current simulation state and the frame about to be
/// simulated, and returns this peer's input for it. The arena bot uses the
/// state; the scripted bot cannot see the state at all (no ROM offsets -- see
/// the module docs in `rollback_libretro::script`) and ignores it.
fn session_loop<S, F>(runner: SessionRunner<S>, frame_budget: u64, next_input: F) -> Result<()>
where
    S: Simulation,
    F: FnMut(&S, u32) -> PlayerInput,
{
    session_loop_observed(runner, frame_budget, next_input, |_| Ok(()))
}

/// The main loop, with a hook that sees every frame that was actually
/// presented.
///
/// The hook fires only on `Advanced`, never on a stall and never during
/// re-simulation, which is what makes a recording made through it an honest
/// record of what reached the player.
fn session_loop_observed<S, F, O>(
    mut runner: SessionRunner<S>,
    frame_budget: u64,
    mut next_input: F,
    mut on_presented: O,
) -> Result<()>
where
    S: Simulation,
    F: FnMut(&S, u32) -> PlayerInput,
    O: FnMut(&S) -> Result<()>,
{
    let mut frames = 0u64;
    let mut last_report = std::time::Instant::now();

    loop {
        let frame = runner.session().current_frame().max(0) as u32;
        let input = next_input(runner.simulation(), frame);

        match runner.step(input)? {
            StepOutcome::Advanced { .. } => {
                frames += 1;
                on_presented(runner.simulation())?;
            }
            StepOutcome::Stalled { .. } => {}
            StepOutcome::Ended(reason) => {
                eprintln!("session ended: {reason:?}");
                break;
            }
        }
        if frames >= frame_budget {
            eprintln!("frame budget reached ({frames} frames)");
            break;
        }
        if last_report.elapsed() >= Duration::from_secs(10) {
            last_report = std::time::Instant::now();
            report(&runner);
        }
        runner.pace();
    }

    finish(runner)
}

fn report<S: Simulation>(runner: &SessionRunner<S>) {
    let s = runner.snapshot();
    eprintln!(
        "t={:>6.1}s frame={:>6} confirmed={:>6} depth={} rollbacks={} acc={:.3} srtt={:.1}ms loss={:.2}%",
        s.elapsed_ms as f64 / 1000.0,
        s.frame,
        s.confirmed_frame,
        s.prediction_depth,
        s.local.rollbacks,
        s.local.prediction_accuracy(),
        s.link.srtt_ms(),
        s.link.loss_ratio() * 100.0,
    );
}

fn finish<S: Simulation>(runner: SessionRunner<S>) -> Result<()> {
    let desync = runner.snapshot().desync;
    report(&runner);
    if let Some(path) = runner.finish(if desync { "desync" } else { "normal" })? {
        eprintln!("log: {}", path.display());
    }
    if desync {
        bail!("session ended in a confirmed desync");
    }
    Ok(())
}
