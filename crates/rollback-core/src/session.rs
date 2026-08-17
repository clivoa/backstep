//! The rollback session: prediction, state history, re-simulation, desync.
//!
//! # How a frame goes by
//!
//! 1. The caller reads the local controller and calls [`RollbackSession::add_local_input`].
//!    The input is filed against frame `current + input_delay`, not the current
//!    frame -- that is the whole of "input delay".
//! 2. The caller calls [`RollbackSession::advance`]. If the remote input for the
//!    current frame has arrived, it is used. If it has not, the session
//!    *predicts* it (repeat the last confirmed remote input) and marks the frame.
//! 3. When the real input eventually arrives via [`RollbackSession::add_remote_inputs`],
//!    the session compares it against what it guessed. On a mismatch it loads
//!    the saved state of the first divergent frame and replays every frame
//!    since, in `Resimulate` mode, back up to the present.
//!
//! # Why the state buffer must be deeper than the prediction limit
//!
//! Prediction is capped at `prediction_limit` frames ahead of the last confirmed
//! remote frame, so the furthest back a rollback can ever reach is
//! `prediction_limit` frames. The saved state *at* that frame has to still be in
//! the ring, hence `state_history > prediction_limit` (enforced in
//! [`SessionConfig::validate`]).

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use crate::config::{ConfigError, Frame, SessionConfig, NULL_FRAME};
use crate::input::{PlayerHandle, PlayerInput};
use crate::simulation::{OutputMode, Simulation, SimulationError};
use crate::stats::{SessionEvent, SessionStats};

/// How much frame history to keep around for logging and late checksums,
/// beyond what rollback itself needs.
const LEDGER_SLACK_FRAMES: Frame = 1_200;

/// A snapshot of the simulation taken at the *start* of `frame`.
#[derive(Clone)]
struct SavedState {
    frame: Frame,
    data: Vec<u8>,
}

/// What we actually fed the simulation for a remote frame.
#[derive(Clone, Copy, Debug)]
struct UsedInput {
    input: PlayerInput,
    /// True if this frame was ever simulated with a guess, even if a later
    /// re-simulation replaced the guess with the confirmed value.
    was_predicted: bool,
}

/// Result of asking the session to move forward one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvanceOutcome {
    /// The frame was simulated and should be presented.
    Advanced { frame: Frame, predicted: bool },
    /// The prediction window is full. The caller must wait for the peer;
    /// no frame was simulated and no state was consumed.
    Stalled { frame: Frame, waiting_for: Frame },
}

/// Why a session stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// Confirmed checksums disagreed.
    Desync { frame: Frame },
    /// No authenticated datagram arrived within the timeout.
    PeerTimeout,
    /// The peer asked to disconnect, or the local side finished normally.
    Closed,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("no local input queued for frame {0}")]
    MissingLocalInput(Frame),
    #[error("cannot roll back to frame {frame}: oldest saved state is {oldest}")]
    HistoryExhausted { frame: Frame, oldest: Frame },
    #[error("desync at confirmed frame {frame}: local {local:#018x} != remote {remote:#018x}")]
    Desync { frame: Frame, local: u64, remote: u64 },
    #[error("peer went silent for {elapsed_ms} ms (limit {limit_ms} ms)")]
    PeerTimeout { elapsed_ms: u64, limit_ms: u32 },
    #[error("peer sent two different inputs for frame {frame}")]
    PeerContradiction { frame: Frame },
    #[error("local input for frame {frame} was queued twice with different values")]
    LocalInputRefiled { frame: Frame },
    #[error("session already ended: {0:?}")]
    Ended(EndReason),
    #[error(transparent)]
    Simulation(#[from] SimulationError),
    #[error(transparent)]
    Config(#[from] ConfigError),
}

pub struct RollbackSession<S: Simulation> {
    sim: S,
    config: SessionConfig,
    local: PlayerHandle,

    /// Next frame to be simulated.
    current_frame: Frame,
    /// Local inputs, already shifted by `input_delay`.
    local_inputs: BTreeMap<Frame, PlayerInput>,
    /// Remote inputs confirmed by the peer.
    remote_inputs: BTreeMap<Frame, PlayerInput>,
    /// What the simulation was actually driven with, per frame.
    used_remote: BTreeMap<Frame, UsedInput>,
    /// Highest frame for which a contiguous run of remote inputs is confirmed.
    remote_confirmed_through: Frame,
    /// Highest frame we have queued a local input for.
    local_queued_through: Frame,
    /// Lowest frame whose prediction has not been checked against reality yet.
    first_unverified: Frame,

    states: VecDeque<SavedState>,
    /// Checksum of the state at the start of each simulated frame.
    frame_checksums: BTreeMap<Frame, u64>,
    /// Highest frame whose checksum has already been handed to the caller.
    checksums_emitted_through: Frame,

    stats: SessionStats,
    events: Vec<SessionEvent>,
    ended: Option<EndReason>,
}

impl<S: Simulation> RollbackSession<S> {
    /// Start a session. `local` says which side this process drives.
    ///
    /// The first `input_delay` frames of *both* players are pre-filled with the
    /// neutral input: by construction neither peer can have produced a real
    /// input for them, and pre-filling keeps both sides in agreement without a
    /// special case in the main loop.
    pub fn new(sim: S, config: SessionConfig, local: PlayerHandle) -> Result<Self, SessionError> {
        config.validate()?;

        let delay = Frame::from(config.input_delay);
        let mut local_inputs = BTreeMap::new();
        let mut remote_inputs = BTreeMap::new();
        for frame in 0..delay {
            local_inputs.insert(frame, PlayerInput::NEUTRAL);
            remote_inputs.insert(frame, PlayerInput::NEUTRAL);
        }

        Ok(Self {
            sim,
            config,
            local,
            current_frame: 0,
            local_inputs,
            remote_inputs,
            used_remote: BTreeMap::new(),
            remote_confirmed_through: delay - 1,
            local_queued_through: delay - 1,
            first_unverified: 0,
            states: VecDeque::with_capacity(usize::from(config.state_history)),
            frame_checksums: BTreeMap::new(),
            checksums_emitted_through: NULL_FRAME,
            stats: SessionStats::default(),
            events: Vec::new(),
            ended: None,
        })
    }

    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    pub fn local_player(&self) -> PlayerHandle {
        self.local
    }

    /// The frame that will be simulated next.
    pub fn current_frame(&self) -> Frame {
        self.current_frame
    }

    /// Highest frame whose inputs are all known -- nothing before it can change.
    pub fn confirmed_frame(&self) -> Frame {
        self.remote_confirmed_through.min(self.current_frame - 1)
    }

    /// How far ahead of the peer we are currently speculating.
    pub fn prediction_depth(&self) -> u32 {
        (self.current_frame - self.remote_confirmed_through - 1).max(0) as u32
    }

    pub fn stats(&self) -> &SessionStats {
        &self.stats
    }

    pub fn simulation(&self) -> &S {
        &self.sim
    }

    pub fn end_reason(&self) -> Option<EndReason> {
        self.ended
    }

    /// Take the events accumulated since the last call, for the JSONL log.
    pub fn drain_events(&mut self) -> Vec<SessionEvent> {
        std::mem::take(&mut self.events)
    }

    /// Mark the session finished without an error (peer `Disconnect`, or the
    /// benchmark reaching its frame budget).
    pub fn close(&mut self) {
        self.ended.get_or_insert(EndReason::Closed);
    }

    /// True when [`RollbackSession::advance`] would refuse to move.
    ///
    /// Callers check this *before* queueing a local input: queueing one while
    /// stalled would refile the same frame on the next tick, and if the first
    /// value had already gone out on the wire the peer would see two different
    /// inputs for one frame and reject the session.
    pub fn would_stall(&self) -> bool {
        !self.remote_inputs.contains_key(&self.current_frame)
            && self.prediction_depth() >= u32::from(self.config.prediction_limit)
    }

    /// Queue the local input for frame `current + input_delay`.
    ///
    /// Returns the frame the input was filed against, which is what the caller
    /// must send to the peer.
    pub fn add_local_input(&mut self, input: PlayerInput) -> Result<Frame, SessionError> {
        self.ensure_running()?;
        let frame = self.current_frame + Frame::from(self.config.input_delay);
        if let Some(&existing) = self.local_inputs.get(&frame) {
            if existing != input {
                // The frame may already have been sent. Changing it now would
                // desynchronise us from our own peer's copy of our inputs.
                return Err(SessionError::LocalInputRefiled { frame });
            }
        }
        self.local_inputs.insert(frame, input);
        self.local_queued_through = self.local_queued_through.max(frame);
        Ok(frame)
    }

    /// Local inputs from `from` onwards, for the redundant `InputBatch` payload.
    pub fn local_inputs_since(&self, from: Frame, max: usize) -> Vec<(Frame, PlayerInput)> {
        self.local_inputs
            .range(from.max(0)..)
            .take(max)
            .map(|(&f, &i)| (f, i))
            .collect()
    }

    pub fn local_queued_through(&self) -> Frame {
        self.local_queued_through
    }

    pub fn remote_confirmed_through(&self) -> Frame {
        self.remote_confirmed_through
    }

    /// Absorb a batch of remote inputs and roll back if any of them contradicts
    /// a prediction we already acted on.
    ///
    /// Duplicated and reordered batches are harmless: re-inserting a value we
    /// already have is a no-op, and inputs for frames we have not reached yet
    /// simply sit in the map until they are needed. A batch that claims a
    /// *different* value for a frame the peer already confirmed is a protocol
    /// violation, not a network artefact, so it is rejected.
    pub fn add_remote_inputs(
        &mut self,
        start_frame: Frame,
        inputs: &[PlayerInput],
    ) -> Result<(), SessionError> {
        self.ensure_running()?;

        for (offset, &input) in inputs.iter().enumerate() {
            let frame = start_frame + offset as Frame;
            if frame < 0 {
                continue;
            }
            match self.remote_inputs.get(&frame) {
                Some(&existing) if existing != input => {
                    return Err(SessionError::PeerContradiction { frame });
                }
                Some(_) => {}
                None => {
                    self.remote_inputs.insert(frame, input);
                }
            }
        }

        self.recompute_remote_confirmed();
        self.reconcile()?;
        self.prune();
        Ok(())
    }

    /// Simulate and present one frame, or report that we must wait.
    pub fn advance(&mut self) -> Result<AdvanceOutcome, SessionError> {
        self.ensure_running()?;

        let frame = self.current_frame;
        let confirmed = self.remote_inputs.get(&frame).copied();

        if confirmed.is_none() && self.prediction_depth() >= u32::from(self.config.prediction_limit)
        {
            // The window is full: speculating further would mean keeping a
            // state we might not be able to roll back to.
            let waiting_for = self.remote_confirmed_through + 1;
            self.stats.stalls += 1;
            self.events.push(SessionEvent::Stalled { frame, waiting_for });
            return Ok(AdvanceOutcome::Stalled { frame, waiting_for });
        }

        let predicted = confirmed.is_none();
        self.step(OutputMode::Present)?;
        self.stats.frames_presented += 1;
        self.events.push(SessionEvent::Advanced { frame, predicted });
        Ok(AdvanceOutcome::Advanced { frame, predicted })
    }

    /// Advance exactly one frame, saving the pre-frame state first.
    fn step(&mut self, mode: OutputMode) -> Result<(), SessionError> {
        let frame = self.current_frame;

        let local = *self
            .local_inputs
            .get(&frame)
            .ok_or(SessionError::MissingLocalInput(frame))?;

        let (remote, is_prediction) = match self.remote_inputs.get(&frame) {
            Some(&input) => (input, false),
            None => (self.predict_remote(), true),
        };

        // Record the guess before it is consumed, so `reconcile` can audit it.
        let entry = self
            .used_remote
            .entry(frame)
            .or_insert(UsedInput { input: remote, was_predicted: false });
        entry.input = remote;
        if is_prediction && !entry.was_predicted {
            entry.was_predicted = true;
            self.stats.predicted_frames += 1;
        }

        let started = Instant::now();
        let data = self.sim.save_state();
        self.stats.save_state_nanos += started.elapsed().as_nanos() as u64;
        self.stats.record_state_size(data.len());

        self.frame_checksums.insert(frame, self.sim.checksum());
        self.push_state(SavedState { frame, data });

        let mut inputs = [PlayerInput::NEUTRAL; 2];
        inputs[self.local.index()] = local;
        inputs[self.local.other().index()] = remote;

        let started = Instant::now();
        self.sim.advance_frame(inputs, mode);
        self.stats.advance_nanos += started.elapsed().as_nanos() as u64;

        self.current_frame += 1;
        Ok(())
    }

    /// The prediction rule: assume the peer keeps doing what it last did.
    ///
    /// For a fighting game this is far better than assuming neutral -- held
    /// directions and charged buttons span many frames, so "unchanged" is right
    /// most of the time, and it is right *especially* during the long neutral
    /// stretches where a misprediction would be most visible.
    fn predict_remote(&self) -> PlayerInput {
        self.remote_inputs
            .get(&self.remote_confirmed_through)
            .copied()
            .unwrap_or(PlayerInput::NEUTRAL)
    }

    fn push_state(&mut self, state: SavedState) {
        if self.states.len() >= usize::from(self.config.state_history) {
            self.states.pop_front();
        }
        self.states.push_back(state);
    }

    fn recompute_remote_confirmed(&mut self) {
        while self
            .remote_inputs
            .contains_key(&(self.remote_confirmed_through + 1))
        {
            self.remote_confirmed_through += 1;
        }
    }

    /// Walk forward from the first unchecked frame; roll back at the first
    /// frame where reality disagreed with our guess, then keep walking.
    ///
    /// The loop matters: a single batch can confirm several frames at once and
    /// contain more than one misprediction. Rolling back to the *earliest*
    /// divergence and replaying fixes all later ones in the same pass, but the
    /// re-simulation may itself introduce fresh guesses further ahead, so we
    /// re-verify until the frontier stops moving.
    fn reconcile(&mut self) -> Result<(), SessionError> {
        loop {
            let mut frame = self.first_unverified;
            let mut divergence = None;

            while frame < self.current_frame {
                let Some(&actual) = self.remote_inputs.get(&frame) else {
                    // Not confirmed yet: everything after it is unverifiable.
                    break;
                };
                match self.used_remote.get(&frame) {
                    Some(used) if used.input != actual => {
                        divergence = Some(frame);
                        break;
                    }
                    _ => frame += 1,
                }
            }

            self.first_unverified = frame;

            let Some(target) = divergence else {
                return Ok(());
            };

            self.stats.mispredicted_frames += 1;
            self.rollback_to(target)?;
        }
    }

    /// Restore the saved state at `frame` and replay up to the present.
    fn rollback_to(&mut self, frame: Frame) -> Result<(), SessionError> {
        let state = self
            .states
            .iter()
            .find(|s| s.frame == frame)
            .cloned()
            .ok_or_else(|| SessionError::HistoryExhausted {
                frame,
                oldest: self.states.front().map_or(NULL_FRAME, |s| s.frame),
            })?;

        let resume_at = self.current_frame;
        let depth = (resume_at - frame).max(0) as u32;

        let started = Instant::now();
        self.sim.load_state(&state.data)?;
        self.stats.load_state_nanos += started.elapsed().as_nanos() as u64;

        self.current_frame = frame;
        // Every state from here on is about to be regenerated with the
        // corrected inputs, so drop the stale ones rather than let a later
        // rollback restore a state built on a wrong guess.
        self.states.retain(|s| s.frame < frame);

        while self.current_frame < resume_at {
            self.step(OutputMode::Resimulate)?;
        }

        self.stats.record_rollback(depth);
        self.events.push(SessionEvent::RolledBack {
            from: resume_at,
            to: frame,
            depth,
        });
        Ok(())
    }

    /// Checksums of frames that can no longer change, on the agreed interval.
    ///
    /// The state at the start of frame `f` is final once every input before `f`
    /// is confirmed, which is exactly `remote_confirmed_through >= f - 1`.
    pub fn confirmed_checksums(&mut self) -> Vec<(Frame, u64)> {
        let final_through = (self.remote_confirmed_through + 1).min(self.current_frame);
        let interval = self.config.checksum_interval as Frame;
        let mut out = Vec::new();

        let mut frame = if self.checksums_emitted_through == NULL_FRAME {
            0
        } else {
            // Next multiple of `interval` strictly after what we already sent.
            (self.checksums_emitted_through / interval + 1) * interval
        };

        while frame <= final_through {
            if let Some(&checksum) = self.frame_checksums.get(&frame) {
                out.push((frame, checksum));
                self.checksums_emitted_through = frame;
            }
            frame += interval;
        }
        out
    }

    /// Compare a checksum the peer computed for a confirmed frame.
    ///
    /// A mismatch is terminal: the two simulations have already diverged and
    /// every frame after this point is fiction. Frames whose checksum we no
    /// longer hold are ignored rather than guessed at.
    pub fn verify_peer_checksum(&mut self, frame: Frame, remote: u64) -> Result<(), SessionError> {
        let Some(&local) = self.frame_checksums.get(&frame) else {
            return Ok(());
        };
        self.stats.checksums_compared += 1;

        if local == remote {
            self.events.push(SessionEvent::ChecksumMatched {
                frame,
                checksum: local,
            });
            return Ok(());
        }

        self.events.push(SessionEvent::Desync {
            frame,
            local,
            remote,
        });
        self.ended = Some(EndReason::Desync { frame });
        Err(SessionError::Desync {
            frame,
            local,
            remote,
        })
    }

    /// End the session if the peer has been silent for too long.
    ///
    /// `elapsed_ms` is the time since the last *authenticated* datagram; the
    /// transport is what decides which datagrams count.
    pub fn check_peer_timeout(&mut self, elapsed_ms: u64) -> Result<(), SessionError> {
        if elapsed_ms < u64::from(self.config.peer_timeout_ms) {
            return Ok(());
        }
        self.ended = Some(EndReason::PeerTimeout);
        Err(SessionError::PeerTimeout {
            elapsed_ms,
            limit_ms: self.config.peer_timeout_ms,
        })
    }

    fn ensure_running(&self) -> Result<(), SessionError> {
        match self.ended {
            Some(reason) => Err(SessionError::Ended(reason)),
            None => Ok(()),
        }
    }

    /// Drop history that no rollback and no pending checksum can reach.
    fn prune(&mut self) {
        let horizon = self.first_unverified.min(self.confirmed_frame()) - LEDGER_SLACK_FRAMES;
        if horizon <= 0 {
            return;
        }
        self.local_inputs = self.local_inputs.split_off(&horizon);
        self.remote_inputs = self.remote_inputs.split_off(&horizon);
        self.used_remote = self.used_remote.split_off(&horizon);
        self.frame_checksums = self.frame_checksums.split_off(&horizon);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::CounterSim;

    fn session(config: SessionConfig) -> RollbackSession<CounterSim> {
        RollbackSession::new(CounterSim::default(), config, PlayerHandle::P1).unwrap()
    }

    fn drive(s: &mut RollbackSession<CounterSim>, frames: usize, input: PlayerInput) {
        for _ in 0..frames {
            s.add_local_input(input).unwrap();
            if !matches!(s.advance().unwrap(), AdvanceOutcome::Advanced { .. }) {
                return;
            }
        }
    }

    #[test]
    fn input_delay_files_inputs_into_the_future() {
        let mut s = session(SessionConfig::default());
        let frame = s.add_local_input(PlayerInput(0xAB)).unwrap();
        assert_eq!(frame, 1, "delay 1 means frame 0's input lands on frame 1");
        // Frame 0 still runs, on the pre-filled neutral input.
        s.advance().unwrap();
        assert_eq!(s.current_frame(), 1);
    }

    #[test]
    fn advancing_without_a_local_input_is_an_error() {
        let mut s = session(SessionConfig {
            input_delay: 0,
            ..Default::default()
        });
        // With zero delay nothing is pre-filled, so frame 0 has no input.
        assert!(matches!(
            s.advance(),
            Err(SessionError::MissingLocalInput(0))
        ));
    }

    #[test]
    fn a_correct_prediction_never_rolls_back() {
        let mut s = session(SessionConfig::default());
        // The peer holds right the whole time, and we know its first input.
        s.add_remote_inputs(1, &[PlayerInput(0x08)]).unwrap();
        drive(&mut s, 6, PlayerInput(0x01));

        let confirmations: Vec<PlayerInput> = (0..6).map(|_| PlayerInput(0x08)).collect();
        s.add_remote_inputs(2, &confirmations).unwrap();

        assert_eq!(s.stats().rollbacks, 0);
        assert!(s.stats().predicted_frames > 0, "it did have to guess");
        assert_eq!(s.stats().mispredicted_frames, 0);
        assert_eq!(s.stats().prediction_accuracy(), 1.0);
    }

    #[test]
    fn a_wrong_prediction_rolls_back_to_the_divergent_frame() {
        let mut s = session(SessionConfig::default());
        drive(&mut s, 5, PlayerInput(0x01));
        assert_eq!(s.current_frame(), 5);

        // Frames 1..4 were guessed as neutral; the peer actually pressed on 3.
        s.add_remote_inputs(
            1,
            &[
                PlayerInput::NEUTRAL,
                PlayerInput::NEUTRAL,
                PlayerInput(0x10),
                PlayerInput(0x10),
            ],
        )
        .unwrap();

        assert_eq!(s.stats().rollbacks, 1);
        assert_eq!(s.stats().last_rollback_depth, 2, "rolled back frames 3 and 4");
        assert_eq!(s.current_frame(), 5, "and caught back up to the present");
    }

    #[test]
    fn multiple_divergences_in_one_batch_are_all_repaired() {
        let mut s = session(SessionConfig {
            prediction_limit: 12,
            state_history: 24,
            ..Default::default()
        });
        drive(&mut s, 10, PlayerInput(0x01));

        // Guessed neutral throughout; reality alternates, diverging repeatedly.
        let actual: Vec<PlayerInput> = (1..10)
            .map(|f| if f % 2 == 0 { PlayerInput(0x20) } else { PlayerInput::NEUTRAL })
            .collect();
        s.add_remote_inputs(1, &actual).unwrap();

        assert!(s.stats().rollbacks >= 1);
        assert_eq!(s.current_frame(), 10);
        // Every frame is now verified against reality.
        assert_eq!(s.first_unverified, 10);
    }

    #[test]
    fn resimulation_reaches_the_same_state_as_a_clean_run() {
        let config = SessionConfig::default();
        let remote: Vec<PlayerInput> = (0..40).map(|f| PlayerInput((f % 5) as u16)).collect();

        // A: everything known up front, no prediction, no rollback.
        let mut clean = session(config);
        clean.add_remote_inputs(1, &remote).unwrap();
        drive(&mut clean, 40, PlayerInput(0x03));

        // B: the same inputs, but always arriving three frames late, so almost
        // every frame is simulated on a guess first.
        let mut ragged = session(config);
        let mut delivered_through: Frame = 0; // frame 0 comes from the pre-fill
        for f in 0..40 {
            ragged.add_local_input(PlayerInput(0x03)).unwrap();
            assert!(matches!(
                ragged.advance().unwrap(),
                AdvanceOutcome::Advanced { .. }
            ));
            let target = f as Frame - 3;
            if target > delivered_through {
                let start = delivered_through + 1;
                let slice = &remote[(start - 1) as usize..target as usize];
                ragged.add_remote_inputs(start, slice).unwrap();
                delivered_through = target;
            }
        }
        // Flush the tail so the last few speculated frames get corrected too.
        ragged
            .add_remote_inputs(delivered_through + 1, &remote[delivered_through as usize..40])
            .unwrap();

        assert!(ragged.stats().rollbacks > 0, "the ragged run must have rolled back");
        assert_eq!(
            clean.simulation().checksum(),
            ragged.simulation().checksum(),
            "rollback must converge on the same state as a clean run"
        );
    }

    #[test]
    fn the_session_stalls_at_the_prediction_limit() {
        let config = SessionConfig::default();
        let mut s = session(config);
        // Peer is silent from frame 1 on. Confirmed through frame 0 (pre-fill).
        for _ in 0..40 {
            s.add_local_input(PlayerInput(0x01)).unwrap();
            if let AdvanceOutcome::Stalled { frame, waiting_for } = s.advance().unwrap() {
                assert_eq!(waiting_for, 1);
                assert_eq!(frame, Frame::from(config.prediction_limit) + 1);
                assert_eq!(s.prediction_depth(), u32::from(config.prediction_limit));
                assert!(s.stats().stalls >= 1);
                return;
            }
        }
        panic!("the session never stalled");
    }

    #[test]
    fn a_stall_clears_once_the_peer_catches_up() {
        let mut s = session(SessionConfig::default());
        for _ in 0..20 {
            s.add_local_input(PlayerInput(0x01)).unwrap();
            if matches!(s.advance().unwrap(), AdvanceOutcome::Stalled { .. }) {
                break;
            }
        }
        s.add_remote_inputs(1, &[PlayerInput(0x04); 4]).unwrap();
        s.add_local_input(PlayerInput(0x01)).unwrap();
        assert!(matches!(
            s.advance().unwrap(),
            AdvanceOutcome::Advanced { .. }
        ));
    }

    #[test]
    fn duplicate_and_reordered_batches_are_absorbed_silently() {
        let mut s = session(SessionConfig::default());
        drive(&mut s, 4, PlayerInput(0x01));

        let batch = [PlayerInput(0x02), PlayerInput(0x02), PlayerInput(0x02)];
        s.add_remote_inputs(1, &batch).unwrap();
        let after_first = (s.stats().rollbacks, s.confirmed_frame());

        s.add_remote_inputs(1, &batch).unwrap(); // exact duplicate
        s.add_remote_inputs(1, &batch[..1]).unwrap(); // stale, reordered
        assert_eq!((s.stats().rollbacks, s.confirmed_frame()), after_first);
    }

    #[test]
    fn contradicting_a_confirmed_frame_is_rejected() {
        let mut s = session(SessionConfig::default());
        s.add_remote_inputs(1, &[PlayerInput(0x02)]).unwrap();
        assert!(matches!(
            s.add_remote_inputs(1, &[PlayerInput(0x99)]),
            Err(SessionError::PeerContradiction { frame: 1 })
        ));
    }

    #[test]
    fn future_inputs_arriving_early_are_kept_for_later() {
        let mut s = session(SessionConfig::default());
        s.add_remote_inputs(30, &[PlayerInput(0x77)]).unwrap();
        // Nothing is confirmed yet: frame 30 is not contiguous with frame 0.
        assert_eq!(s.remote_confirmed_through(), 0);
        drive(&mut s, 3, PlayerInput(0x01));
        assert_eq!(s.stats().rollbacks, 0);
    }

    #[test]
    fn checksums_are_emitted_once_per_interval_and_only_when_final() {
        let config = SessionConfig {
            checksum_interval: 4,
            prediction_limit: 8,
            state_history: 16,
            ..Default::default()
        };
        let mut s = session(config);
        s.add_remote_inputs(1, &[PlayerInput(0x01); 12]).unwrap();
        drive(&mut s, 10, PlayerInput(0x01));

        let first = s.confirmed_checksums();
        let frames: Vec<Frame> = first.iter().map(|&(f, _)| f).collect();
        assert_eq!(frames, vec![0, 4, 8]);
        assert!(s.confirmed_checksums().is_empty(), "no frame is emitted twice");
    }

    #[test]
    fn a_matching_peer_checksum_keeps_the_session_alive() {
        let mut s = session(SessionConfig::default());
        s.add_remote_inputs(1, &[PlayerInput(0x01); 4]).unwrap();
        drive(&mut s, 3, PlayerInput(0x01));
        let (frame, checksum) = s.confirmed_checksums()[0];
        s.verify_peer_checksum(frame, checksum).unwrap();
        assert_eq!(s.stats().checksums_compared, 1);
        assert!(s.end_reason().is_none());
    }

    #[test]
    fn a_mismatching_peer_checksum_ends_the_session_immediately() {
        let mut s = session(SessionConfig::default());
        s.add_remote_inputs(1, &[PlayerInput(0x01); 4]).unwrap();
        drive(&mut s, 3, PlayerInput(0x01));
        let (frame, checksum) = s.confirmed_checksums()[0];

        assert!(matches!(
            s.verify_peer_checksum(frame, checksum ^ 0xFF),
            Err(SessionError::Desync { .. })
        ));
        assert_eq!(s.end_reason(), Some(EndReason::Desync { frame }));
        assert!(matches!(s.advance(), Err(SessionError::Ended(_))));
    }

    #[test]
    fn an_unknown_checksum_frame_is_ignored_rather_than_guessed() {
        let mut s = session(SessionConfig::default());
        s.verify_peer_checksum(999_999, 0xDEAD).unwrap();
        assert_eq!(s.stats().checksums_compared, 0);
        assert!(s.end_reason().is_none());
    }

    #[test]
    fn silence_past_the_timeout_ends_the_session() {
        let mut s = session(SessionConfig::default());
        s.check_peer_timeout(2_999).unwrap();
        assert!(matches!(
            s.check_peer_timeout(3_000),
            Err(SessionError::PeerTimeout { .. })
        ));
        assert_eq!(s.end_reason(), Some(EndReason::PeerTimeout));
    }

    #[test]
    fn rolling_back_past_the_state_buffer_is_reported_not_silently_wrong() {
        // Deliberately misconfigured through the back door: a deep buffer is
        // what protects us, so prove the guard fires when history is gone.
        let mut s = session(SessionConfig {
            prediction_limit: 8,
            state_history: 10,
            ..Default::default()
        });
        s.add_remote_inputs(1, &[PlayerInput::NEUTRAL; 60]).unwrap();
        drive(&mut s, 40, PlayerInput(0x01));
        assert!(matches!(
            s.rollback_to(0),
            Err(SessionError::HistoryExhausted { frame: 0, .. })
        ));
    }

    #[test]
    fn local_inputs_since_feeds_the_redundant_batch() {
        let mut s = session(SessionConfig::default());
        drive(&mut s, 12, PlayerInput(0x05));
        let batch = s.local_inputs_since(s.local_queued_through() - 7, 8);
        assert_eq!(batch.len(), 8, "the wire format repeats the last eight inputs");
        assert!(batch.windows(2).all(|w| w[1].0 == w[0].0 + 1));
    }
}
