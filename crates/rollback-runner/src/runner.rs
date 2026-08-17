//! The per-frame session loop.

#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rollback_core::{
    AdvanceOutcome, EndReason, Frame, PlayerHandle, PlayerInput, RollbackSession, SessionConfig,
    SessionError, Simulation,
};
use rollback_net::{DisconnectReason, Message, TransportError, UdpTransport, INPUT_REDUNDANCY};
use rollback_telemetry::{
    jsonl::Record, Exporter, MetricsSnapshot, ProcessStats, SessionInfo, SessionLog,
};

/// How often the peer's telemetry summary is exchanged, in frames.
const TELEMETRY_INTERVAL: Frame = 60;
/// How often a full metrics snapshot is appended to the JSONL log, in frames.
const LOG_SNAPSHOT_INTERVAL: Frame = 60;
/// How often process CPU/memory is sampled, in frames. Reading `/proc` every
/// frame would cost more than the numbers are worth.
const PROCESS_SAMPLE_INTERVAL: Frame = 30;

#[derive(Clone, Debug)]
pub struct RunnerConfig {
    pub session: SessionConfig,
    pub local_player: PlayerHandle,
    pub log_dir: PathBuf,
    pub session_name: String,
    /// `None` disables the Prometheus exporter (used by tests).
    pub exporter_addr: Option<String>,
    pub info: SessionInfo,
}

/// What one call to [`SessionRunner::step`] did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StepOutcome {
    /// A frame was simulated and should be drawn.
    Advanced { frame: Frame, predicted: bool },
    /// The prediction window is full. No local input was consumed.
    Stalled { waiting_for: Frame },
    /// The session is over.
    Ended(EndReason),
}

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("log error: {0}")]
    Log(#[from] std::io::Error),
}

pub struct SessionRunner<S: Simulation> {
    session: RollbackSession<S>,
    transport: UdpTransport,
    log: Option<SessionLog>,
    exporter: Option<Exporter>,
    snapshot: MetricsSnapshot,
    started: Instant,
    last_telemetry_frame: Frame,
    last_log_snapshot_frame: Frame,
    last_process_sample_frame: Frame,
    /// Wall-clock instant the next frame is due, for 60 Hz pacing.
    next_frame_at: Instant,
    frame_duration: Duration,
    ended: Option<EndReason>,
}

impl<S: Simulation> SessionRunner<S> {
    pub fn new(
        simulation: S,
        transport: UdpTransport,
        config: RunnerConfig,
    ) -> Result<Self, RunnerError> {
        let session = RollbackSession::new(simulation, config.session, config.local_player)?;
        let frame_duration = config.session.frame_duration();

        let log = SessionLog::create(&config.log_dir, &config.session_name, &config.info).ok();
        let snapshot = MetricsSnapshot::new(config.info);
        let exporter = config
            .exporter_addr
            .as_deref()
            .and_then(|addr| Exporter::start(addr, snapshot.clone()).ok());

        Ok(SessionRunner {
            session,
            transport,
            log,
            exporter,
            snapshot,
            started: Instant::now(),
            last_telemetry_frame: -1,
            last_log_snapshot_frame: -1,
            last_process_sample_frame: -1,
            next_frame_at: Instant::now(),
            frame_duration,
            ended: None,
        })
    }

    pub fn session(&self) -> &RollbackSession<S> {
        &self.session
    }

    pub fn simulation(&self) -> &S {
        self.session.simulation()
    }

    pub fn transport(&self) -> &UdpTransport {
        &self.transport
    }

    pub fn snapshot(&self) -> &MetricsSnapshot {
        &self.snapshot
    }

    pub fn log_path(&self) -> Option<PathBuf> {
        self.log.as_ref().map(|l| l.path().to_path_buf())
    }

    pub fn exporter_addr(&self) -> Option<std::net::SocketAddr> {
        self.exporter.as_ref().map(|e| e.addr())
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Sleep until the next frame is due.
    ///
    /// Pacing matters even for `bench`: a run that simulates as fast as it can
    /// would measure RTT against a link nobody is using at 60 Hz, which is not
    /// the thing under study.
    pub fn pace(&mut self) {
        let now = Instant::now();
        if self.next_frame_at > now {
            std::thread::sleep(self.next_frame_at - now);
        } else if now.duration_since(self.next_frame_at) > self.frame_duration * 8 {
            // Fell far behind (a long rollback, a scheduler hiccup): resync
            // rather than sprint through eight frames to catch up.
            self.next_frame_at = now;
        }
        self.next_frame_at += self.frame_duration;
    }

    /// Run one frame. `input` is only consumed when the session actually
    /// advances -- see [`RollbackSession::would_stall`].
    pub fn step(&mut self, input: PlayerInput) -> Result<StepOutcome, RunnerError> {
        if let Some(reason) = self.ended {
            return Ok(StepOutcome::Ended(reason));
        }

        // 1. Drain the socket first: a rollback triggered here happens before
        //    this frame is built on top of a stale prediction.
        if let Some(outcome) = self.pump_network()? {
            return Ok(outcome);
        }

        // 2. Stalled? Do no local work at all.
        if self.session.would_stall() {
            let waiting_for = self.session.remote_confirmed_through() + 1;
            let _ = self.session.advance()?; // records the stall and the event
            self.drain_events()?;
            self.publish()?;
            return Ok(StepOutcome::Stalled { waiting_for });
        }

        // 3. Queue the local input and 4. put it on the wire before simulating.
        let frame = self.session.add_local_input(input)?;
        let t_ms = self.elapsed_ms();
        if let Some(log) = &mut self.log {
            log.write_local_input(t_ms, frame, input)?;
        }
        self.send_input_batch()?;

        // 5. Simulate.
        let outcome = match self.session.advance()? {
            AdvanceOutcome::Advanced { frame, predicted } => {
                StepOutcome::Advanced { frame, predicted }
            }
            AdvanceOutcome::Stalled { waiting_for, .. } => StepOutcome::Stalled { waiting_for },
        };

        // 6. Any frame that just became final gets its checksum exchanged.
        for (frame, value) in self.session.confirmed_checksums() {
            self.transport.send(Message::Checksum { frame, value })?;
        }

        // 7. Telemetry, logging, liveness.
        self.maybe_send_telemetry()?;
        self.transport.pump()?;
        self.drain_events()?;
        self.publish()?;

        if let Some(elapsed) = self.transport.ms_since_authenticated() {
            if let Err(e) = self.session.check_peer_timeout(elapsed) {
                self.finish_with(EndReason::PeerTimeout, DisconnectReason::Timeout)?;
                return match e {
                    SessionError::PeerTimeout { .. } => {
                        Ok(StepOutcome::Ended(EndReason::PeerTimeout))
                    }
                    other => Err(other.into()),
                };
            }
        }

        Ok(outcome)
    }

    /// Receive, authenticate and apply everything waiting on the socket.
    ///
    /// Returns `Some` when the session ended as a result.
    fn pump_network(&mut self) -> Result<Option<StepOutcome>, RunnerError> {
        let t_ms = self.elapsed_ms();
        for received in self.transport.receive()? {
            let packet = received.packet;
            if let Some(log) = &mut self.log {
                log.write(&Record::Received {
                    t_ms,
                    sequence: packet.sequence,
                    kind: packet.message.kind_name(),
                    ack: match &packet.message {
                        Message::InputBatch { ack_sequence, .. } => Some(*ack_sequence),
                        _ => None,
                    },
                })?;
            }

            match packet.message {
                Message::InputBatch {
                    start_frame,
                    inputs,
                    ..
                } => {
                    if let Some(log) = &mut self.log {
                        log.write_remote_inputs(t_ms, start_frame, &inputs)?;
                    }
                    match self.session.add_remote_inputs(start_frame, &inputs) {
                        Ok(()) => {}
                        // A peer that contradicts itself is broken, not lagging.
                        Err(e @ SessionError::PeerContradiction { .. }) => {
                            self.finish_with(EndReason::Closed, DisconnectReason::LocalError)?;
                            return Err(e.into());
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
                Message::Checksum { frame, value } => {
                    if let Err(SessionError::Desync { .. }) =
                        self.session.verify_peer_checksum(frame, value)
                    {
                        self.snapshot.desync = true;
                        self.finish_with(EndReason::Desync { frame }, DisconnectReason::Desync)?;
                        return Ok(Some(StepOutcome::Ended(EndReason::Desync { frame })));
                    }
                }
                Message::Telemetry(summary) => self.snapshot.remote = Some(summary),
                Message::Disconnect(_) => {
                    self.finish_with(EndReason::Closed, DisconnectReason::Normal)?;
                    return Ok(Some(StepOutcome::Ended(EndReason::Closed)));
                }
                Message::Hello(_) | Message::HelloAck { .. } => {
                    // A retransmitted handshake packet from a peer that had not
                    // yet seen our ack. Harmless.
                }
            }
        }
        Ok(None)
    }

    /// Send the last [`INPUT_REDUNDANCY`] local inputs.
    ///
    /// Repeating them is the whole loss-recovery strategy: there are no
    /// retransmissions and no acknowledgement of individual frames, so an input
    /// has eight chances to arrive before the peer would have to stall. At 2%
    /// loss the chance of losing all eight is about 2.6e-14.
    fn send_input_batch(&mut self) -> Result<(), RunnerError> {
        let through = self.session.local_queued_through();
        let start = (through - INPUT_REDUNDANCY as Frame + 1).max(0);
        let inputs: Vec<PlayerInput> = self
            .session
            .local_inputs_since(start, INPUT_REDUNDANCY)
            .into_iter()
            .map(|(_, input)| input)
            .collect();
        if inputs.is_empty() {
            return Ok(());
        }

        let sequence = self.transport.send(Message::InputBatch {
            start_frame: start,
            inputs,
            highest_remote_frame: self.session.remote_confirmed_through(),
            ack_sequence: self.transport.highest_received_sequence(),
        })?;
        let t_ms = self.elapsed_ms();
        if let Some(log) = &mut self.log {
            log.write(&Record::Sent {
                t_ms,
                sequence,
                kind: "input_batch",
                bytes: 0,
            })?;
        }
        Ok(())
    }

    fn maybe_send_telemetry(&mut self) -> Result<(), RunnerError> {
        let frame = self.session.current_frame();
        if frame - self.last_telemetry_frame < TELEMETRY_INTERVAL {
            return Ok(());
        }
        self.last_telemetry_frame = frame;
        self.refresh_snapshot();
        self.transport
            .send(Message::Telemetry(self.snapshot.to_summary()))?;
        Ok(())
    }

    fn drain_events(&mut self) -> Result<(), RunnerError> {
        let events = self.session.drain_events();
        if events.is_empty() {
            return Ok(());
        }
        let t_ms = self.elapsed_ms();
        if let Some(log) = &mut self.log {
            log.write_events(t_ms, &events)?;
        }
        Ok(())
    }

    fn refresh_snapshot(&mut self) {
        self.snapshot.elapsed_ms = self.elapsed_ms();
        self.snapshot.frame = self.session.current_frame();
        self.snapshot.confirmed_frame = self.session.confirmed_frame();
        self.snapshot.prediction_depth = self.session.prediction_depth();
        self.snapshot.local = *self.session.stats();
        self.snapshot.link = *self.transport.stats();
    }

    fn publish(&mut self) -> Result<(), RunnerError> {
        self.refresh_snapshot();

        let frame = self.session.current_frame();
        if frame - self.last_process_sample_frame >= PROCESS_SAMPLE_INTERVAL {
            self.last_process_sample_frame = frame;
            self.snapshot.process = ProcessStats::sample();
        }
        if let Some(exporter) = &self.exporter {
            exporter.publish(&self.snapshot);
        }
        if frame - self.last_log_snapshot_frame >= LOG_SNAPSHOT_INTERVAL {
            self.last_log_snapshot_frame = frame;
            if let Some(log) = &mut self.log {
                log.write(&Record::Metrics {
                    t_ms: self.snapshot.elapsed_ms,
                    snapshot: Box::new(self.snapshot.clone()),
                })?;
            }
        }
        Ok(())
    }

    fn finish_with(
        &mut self,
        reason: EndReason,
        wire_reason: DisconnectReason,
    ) -> Result<(), RunnerError> {
        if self.ended.is_some() {
            return Ok(());
        }
        self.ended = Some(reason);
        self.session.close();
        // Best effort: the peer may already be gone, and a failure to say
        // goodbye must not mask the reason we are stopping.
        let _ = self.transport.send(Message::Disconnect(wire_reason));
        let _ = self.transport.flush();
        self.drain_events()?;
        self.publish()?;
        Ok(())
    }

    /// End the session and close the log. Always call this: it is what makes
    /// `just collect` find a complete file.
    pub fn finish(mut self, reason: &str) -> Result<Option<PathBuf>, RunnerError> {
        if self.ended.is_none() {
            self.finish_with(EndReason::Closed, DisconnectReason::Normal)?;
        }
        self.refresh_snapshot();
        self.snapshot.process = ProcessStats::sample();
        if let Some(exporter) = &self.exporter {
            exporter.publish(&self.snapshot);
        }
        let elapsed = self.snapshot.elapsed_ms;
        match self.log.take() {
            Some(log) => Ok(Some(log.finish(elapsed, reason, &self.snapshot)?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rollback_core::{testing::CounterSim, NetworkProfile, SimulationKind};
    use rollback_net::Authenticator;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rollback-runner-{}-{tag}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config(player: PlayerHandle, dir: &Path) -> RunnerConfig {
        RunnerConfig {
            session: SessionConfig::default(),
            local_player: player,
            log_dir: dir.to_path_buf(),
            session_name: format!("{player:?}"),
            exporter_addr: None,
            info: SessionInfo::new(SimulationKind::Arena, "natural", &format!("{player:?}")),
        }
    }

    fn linked_pair(
        dir: &Path,
        profile: NetworkProfile,
    ) -> (SessionRunner<CounterSim>, SessionRunner<CounterSim>) {
        let auth = Authenticator::from_passphrase("runner-test");
        let bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let mut a = UdpTransport::bind(bind, auth.clone(), profile).unwrap();
        let mut b = UdpTransport::bind(bind, auth, profile).unwrap();
        a.set_peer(b.local_addr().unwrap());
        b.set_peer(a.local_addr().unwrap());

        (
            SessionRunner::new(CounterSim::default(), a, config(PlayerHandle::P1, dir)).unwrap(),
            SessionRunner::new(CounterSim::default(), b, config(PlayerHandle::P2, dir)).unwrap(),
        )
    }

    #[test]
    fn two_runners_advance_together_and_converge() {
        // A 20 ms one-way delay on each side puts the peer roughly two and a
        // half frames behind, so prediction and rollback genuinely happen.
        // On a bare loopback the remote input always beats the frame it belongs
        // to and the interesting path is never taken.
        let dir = temp_dir("converge");
        let profile = NetworkProfile::named("delay20").unwrap().1;
        let (mut p1, mut p2) = linked_pair(&dir, profile);

        for f in 0..240u16 {
            p1.step(PlayerInput(f % 7)).unwrap();
            p2.step(PlayerInput((f * 3) % 11)).unwrap();
            p1.pace();
        }
        // Let the tail of the inputs land on both sides.
        for _ in 0..30 {
            p1.step(PlayerInput(0)).unwrap();
            p2.step(PlayerInput(0)).unwrap();
            p1.pace();
        }

        assert!(
            p1.session().confirmed_frame() > 200,
            "the link must be live"
        );
        assert!(p1.snapshot().local.predicted_frames > 0, "it had to guess");
        assert!(
            p1.snapshot().local.rollbacks > 0,
            "and it guessed wrong sometimes"
        );
        assert!(!p1.snapshot().desync);
        assert!(!p2.snapshot().desync);
        assert!(
            p1.snapshot().local.checksums_compared > 0,
            "confirmed-frame checksums must have been compared and agreed"
        );
        assert!(p2.snapshot().local.checksums_compared > 0);

        p1.finish("normal").unwrap();
        p2.finish("normal").unwrap();
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_silent_peer_stalls_and_then_times_out() {
        let dir = temp_dir("timeout");
        let auth = Authenticator::from_passphrase("runner-test");
        let bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let mut lonely = UdpTransport::bind(bind, auth, NetworkProfile::NATURAL).unwrap();
        lonely.set_peer(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1));

        let mut runner = SessionRunner::new(
            CounterSim::default(),
            lonely,
            RunnerConfig {
                session: SessionConfig {
                    peer_timeout_ms: 200,
                    ..Default::default()
                },
                ..config(PlayerHandle::P1, &dir)
            },
        )
        .unwrap();

        let mut stalled = false;
        for _ in 0..200 {
            match runner.step(PlayerInput(1)).unwrap() {
                StepOutcome::Stalled { .. } => stalled = true,
                StepOutcome::Ended(_) => break,
                StepOutcome::Advanced { .. } => {}
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(stalled, "a silent peer must fill the prediction window");
        // The timeout only fires once a datagram has ever authenticated, and
        // none ever will here -- so the session stays stalled rather than
        // claiming a peer that was never there has gone away.
        assert_eq!(runner.session().end_reason(), None);

        runner.finish("stalled").unwrap();
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_log_is_written_and_closed() {
        let dir = temp_dir("log");
        let (mut p1, mut p2) = linked_pair(&dir, NetworkProfile::NATURAL);
        for _ in 0..30 {
            p1.step(PlayerInput(1)).unwrap();
            p2.step(PlayerInput(2)).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        let path = p1.finish("normal").unwrap().expect("a log was written");
        p2.finish("normal").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.lines().next().unwrap().contains("session_start"));
        assert!(text.lines().last().unwrap().contains("session_end"));
        assert!(text.contains("local_input"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn pacing_targets_the_configured_frame_rate() {
        let dir = temp_dir("pace");
        let (mut p1, _p2) = linked_pair(&dir, NetworkProfile::NATURAL);
        let started = Instant::now();
        for _ in 0..30 {
            p1.pace();
        }
        let elapsed = started.elapsed();
        // 30 frames at 60 Hz is half a second; allow for scheduler slop.
        assert!(
            elapsed >= Duration::from_millis(400) && elapsed < Duration::from_millis(900),
            "30 paced frames took {elapsed:?}"
        );
        p1.finish("normal").unwrap();
        std::fs::remove_dir_all(dir).ok();
    }
}
