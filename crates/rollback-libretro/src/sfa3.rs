//! Booting Street Fighter Alpha 3 deterministically, and driving P2.
//!
//! # Why this is a timed macro script and not a memory reader
//!
//! The obvious way to automate a boot is to read the game's RAM: watch a "game
//! mode" byte, wait for it to hit the value that means "character select", then
//! press. That is fast, robust -- and forbidden here, deliberately:
//!
//! * memory offsets are version- and region-specific, so the automation would
//!   silently break on a different ROM revision while the handshake still says
//!   the hashes match;
//! * reading RAM through `retro_get_memory_data` is not part of what the
//!   rollback needs, and building a dependency on it means the lab no longer
//!   demonstrates that rollback works on an *opaque* simulation.
//!
//! So the script is purely time-based: hold a button for N frames, wait M, move
//! on. This is only sound because the emulator is deterministic -- the same
//! script from the same reset always lands on the same screen at the same
//! frame, on both peers, which is the entire premise of the lab.
//!
//! The frame counts below are conservative: each stage waits considerably
//! longer than the animation needs. A boot that takes two seconds too long
//! costs nothing; a boot that presses one frame too early desynchronises the
//! menu state between peers and the match starts with different characters.

use rollback_core::{Button, DeterministicRng, PlayerInput};

/// One step of a macro: hold `input` for `frames` frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub frames: u32,
    pub input: PlayerInput,
}

impl Step {
    pub const fn hold(frames: u32, input: PlayerInput) -> Step {
        Step { frames, input }
    }

    pub const fn wait(frames: u32) -> Step {
        Step {
            frames,
            input: PlayerInput::NEUTRAL,
        }
    }
}

/// A compiled timeline: absolute frame ranges mapped to inputs.
#[derive(Clone, Debug, Default)]
pub struct Macro {
    steps: Vec<Step>,
    total: u32,
}

impl Macro {
    pub fn new(steps: Vec<Step>) -> Macro {
        let total = steps.iter().map(|s| s.frames).sum();
        Macro { steps, total }
    }

    pub fn total_frames(&self) -> u32 {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// Input at `frame` frames into the macro, or `None` once it is over.
    pub fn at(&self, frame: u32) -> Option<PlayerInput> {
        let mut cursor = 0;
        for step in &self.steps {
            if frame < cursor + step.frames {
                return Some(step.input);
            }
            cursor += step.frames;
        }
        None
    }
}

fn press(button: Button) -> PlayerInput {
    PlayerInput::NEUTRAL.with(button)
}

/// Character slots on the SFA3 select grid, as a number of `Right` presses from
/// the default cursor position.
///
/// Named after the intent rather than the actual roster position: the point is
/// that both peers pick the *same* slot, and the slot is fixed in code rather
/// than chosen at runtime.
pub const P1_CURSOR_STEPS: u32 = 0;
pub const P2_CURSOR_STEPS: u32 = 3;

/// Frames to sit on the boot logos before touching anything.
const BOOT_WAIT: u32 = 420; // 7 s
/// Frames to hold a menu button, and to wait afterwards.
const TAP: u32 = 6;
const SETTLE: u32 = 24;
/// Frames to let the character-select intro and the round-start animation run.
const SELECT_INTRO: u32 = 180; // 3 s
const ROUND_INTRO: u32 = 300; // 5 s

/// Build the boot macro for one side.
///
/// Both sides run their own copy at the same absolute frame numbers, so the two
/// peers make identical menu inputs even though each only owns one player.
pub fn boot_macro(cursor_steps: u32) -> Macro {
    let mut steps = vec![Step::wait(BOOT_WAIT)];

    // Insert a coin twice: CPS-2 boards want one credit per player, and a
    // second coin on an already-credited machine is harmless.
    for _ in 0..2 {
        steps.push(Step::hold(TAP, press(Button::Coin)));
        steps.push(Step::wait(SETTLE));
    }

    // Start: attract mode -> mode select -> character select.
    for _ in 0..2 {
        steps.push(Step::hold(TAP, press(Button::Start)));
        steps.push(Step::wait(SETTLE));
    }
    steps.push(Step::wait(SELECT_INTRO));

    // Walk the cursor to the fixed slot.
    for _ in 0..cursor_steps {
        steps.push(Step::hold(TAP, press(Button::Right)));
        steps.push(Step::wait(TAP * 2));
    }

    // Confirm the character, then the ISM/super-arts prompt that follows.
    for _ in 0..2 {
        steps.push(Step::hold(TAP, press(Button::Confirm)));
        steps.push(Step::wait(SETTLE));
    }
    steps.push(Step::wait(ROUND_INTRO));

    Macro::new(steps)
}

/// Drives both players through the boot, then hands P1 to the human.
pub struct Sfa3Director {
    p1: Macro,
    p2: Macro,
    ready_at: u32,
}

impl Default for Sfa3Director {
    fn default() -> Self {
        Self::new()
    }
}

impl Sfa3Director {
    pub fn new() -> Sfa3Director {
        let p1 = boot_macro(P1_CURSOR_STEPS);
        let p2 = boot_macro(P2_CURSOR_STEPS);
        // The two macros differ in length by the cursor walk, so the match is
        // only "ready" once the longer of the two has finished.
        let ready_at = p1.total_frames().max(p2.total_frames());
        Sfa3Director { p1, p2, ready_at }
    }

    /// Frame at which control passes to the players.
    pub fn ready_at(&self) -> u32 {
        self.ready_at
    }

    pub fn is_ready(&self, frame: u32) -> bool {
        frame >= self.ready_at
    }

    /// Scripted input for `player` at `frame`, or `None` once the boot is over.
    ///
    /// Returning `None` is the signal to start reading the real controller;
    /// during the boot the human's inputs are ignored on purpose, so a stray
    /// button press cannot change the character selection on one peer only.
    pub fn input(&self, frame: u32, player: usize) -> Option<PlayerInput> {
        if self.is_ready(frame) {
            return None;
        }
        let script = if player == 0 { &self.p1 } else { &self.p2 };
        Some(script.at(frame).unwrap_or(PlayerInput::NEUTRAL))
    }
}

/// The P2 opponent: a deterministic sequence of macros.
///
/// Like the arena bot, this is a *player*, not part of the simulation. It reads
/// nothing from the game -- it cannot, without ROM offsets -- so it plays a
/// fixed repertoire rather than reacting. That is enough to generate the input
/// churn a rollback experiment needs, which is what it is for.
pub struct Sfa3Bot {
    rng: DeterministicRng,
    repertoire: Vec<Macro>,
    current: usize,
    elapsed: u32,
}

impl Sfa3Bot {
    pub fn new(seed: u64) -> Sfa3Bot {
        Sfa3Bot {
            rng: DeterministicRng::new(seed ^ 0x5FA3_5FA3_5FA3_5FA3),
            repertoire: repertoire(),
            current: 0,
            elapsed: 0,
        }
    }

    /// Next input. Call exactly once per presented frame.
    pub fn decide(&mut self) -> PlayerInput {
        let input = self.repertoire[self.current]
            .at(self.elapsed)
            .unwrap_or(PlayerInput::NEUTRAL);
        self.elapsed += 1;

        if self.elapsed >= self.repertoire[self.current].total_frames() {
            self.current = self.rng.below(self.repertoire.len() as u32) as usize;
            self.elapsed = 0;
        }
        input
    }

    /// Index of the macro currently playing, for the overlay.
    pub fn current_macro(&self) -> usize {
        self.current
    }
}

/// The macros P2 cycles through.
fn repertoire() -> Vec<Macro> {
    let neutral = PlayerInput::NEUTRAL;
    let left = press(Button::Left);
    let right = press(Button::Right);
    let down = press(Button::Down);
    let up = press(Button::Up);

    vec![
        // Walk forward, then jab.
        Macro::new(vec![
            Step::hold(30, left),
            Step::hold(4, press(Button::Attack)),
            Step::wait(20),
        ]),
        // Back off and block low.
        Macro::new(vec![
            Step::hold(24, right),
            Step::hold(40, down.with(Button::Right)),
            Step::wait(10),
        ]),
        // Jump in.
        Macro::new(vec![
            Step::hold(6, up.with(Button::Left)),
            Step::wait(30),
            Step::hold(4, press(Button::Attack)),
            Step::wait(16),
        ]),
        // Quarter-circle forward + punch. Facing left, "forward" is Left.
        Macro::new(vec![
            Step::hold(4, down),
            Step::hold(4, down.with(Button::Left)),
            Step::hold(4, left),
            Step::hold(4, left.with(Button::Attack)),
            Step::wait(30),
        ]),
        // Crouch and poke.
        Macro::new(vec![
            Step::hold(20, down),
            Step::hold(4, down.with(Button::Block)),
            Step::wait(18),
        ]),
        // Stand still: the neutral game matters too, and a bot that never
        // stops moving makes prediction unrealistically hard.
        Macro::new(vec![Step::hold(40, neutral)]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_macro_reports_the_input_of_the_step_it_is_in() {
        let m = Macro::new(vec![
            Step::hold(3, press(Button::Coin)),
            Step::wait(2),
            Step::hold(1, press(Button::Start)),
        ]);
        assert_eq!(m.total_frames(), 6);
        assert_eq!(m.at(0), Some(press(Button::Coin)));
        assert_eq!(m.at(2), Some(press(Button::Coin)));
        assert_eq!(m.at(3), Some(PlayerInput::NEUTRAL));
        assert_eq!(m.at(5), Some(press(Button::Start)));
        assert_eq!(m.at(6), None, "past the end");
    }

    #[test]
    fn an_empty_macro_is_immediately_over() {
        let m = Macro::new(Vec::new());
        assert!(m.is_empty());
        assert_eq!(m.at(0), None);
    }

    #[test]
    fn the_boot_script_presses_coin_before_start() {
        let m = boot_macro(0);
        let first_coin = (0..m.total_frames())
            .find(|&f| m.at(f).unwrap().contains(Button::Coin))
            .expect("must insert a coin");
        let first_start = (0..m.total_frames())
            .find(|&f| m.at(f).unwrap().contains(Button::Start))
            .expect("must press start");
        assert!(first_coin >= BOOT_WAIT, "must let the logos finish");
        assert!(first_coin < first_start, "coin before start");
    }

    #[test]
    fn a_longer_cursor_walk_takes_more_frames() {
        assert!(boot_macro(3).total_frames() > boot_macro(0).total_frames());
    }

    #[test]
    fn the_director_hands_over_only_after_both_scripts_finish() {
        let d = Sfa3Director::new();
        assert_eq!(d.ready_at(), boot_macro(P2_CURSOR_STEPS).total_frames());
        assert!(!d.is_ready(d.ready_at() - 1));
        assert!(d.is_ready(d.ready_at()));

        assert!(d.input(0, 0).is_some());
        assert!(d.input(0, 1).is_some());
        assert_eq!(d.input(d.ready_at(), 0), None, "P1 goes to the human");
        assert_eq!(d.input(d.ready_at(), 1), None, "P2 goes to the bot");
    }

    #[test]
    fn the_two_sides_select_different_characters() {
        let d = Sfa3Director::new();
        let rights = |player: usize| {
            (0..d.ready_at())
                .filter(|&f| {
                    d.input(f, player)
                        .is_some_and(|i| i.contains(Button::Right))
                })
                .count()
        };
        assert_ne!(rights(0), rights(1), "the cursors must end up apart");
    }

    #[test]
    fn the_boot_fits_in_a_reasonable_wait() {
        // Every stage waits longer than the animation needs, so the boot is
        // slow on purpose -- but it still has to be short enough that `just
        // play` is usable, and short enough to fit inside a 180-second session
        // with plenty of match left over.
        let ready_at = Sfa3Director::new().ready_at();
        assert!(ready_at < 60 * 25, "{ready_at} frames is too long a wait");
        assert!(ready_at > 60 * 10, "{ready_at} frames rushes the boot");
    }

    #[test]
    fn the_bot_is_reproducible_from_its_seed() {
        let run = |seed| {
            let mut bot = Sfa3Bot::new(seed);
            (0..3_000).map(|_| bot.decide()).collect::<Vec<_>>()
        };
        assert_eq!(run(11), run(11));
        assert_ne!(run(11), run(12));
    }

    #[test]
    fn the_bot_actually_presses_things() {
        let mut bot = Sfa3Bot::new(5);
        let inputs: Vec<PlayerInput> = (0..3_000).map(|_| bot.decide()).collect();
        assert!(inputs.iter().any(|i| i.contains(Button::Attack)));
        assert!(inputs.iter().any(|i| i.contains(Button::Left)));
        assert!(inputs.contains(&PlayerInput::NEUTRAL));
    }

    #[test]
    fn the_bot_never_holds_opposing_directions() {
        let mut bot = Sfa3Bot::new(9);
        for _ in 0..10_000 {
            let i = bot.decide();
            let horizontal = i.contains(Button::Left) && i.contains(Button::Right);
            assert!(!horizontal, "left and right held together");
            let vertical = i.contains(Button::Up) && i.contains(Button::Down);
            assert!(!vertical, "up and down held together");
        }
    }
}
