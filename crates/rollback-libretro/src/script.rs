//! Booting an arcade game deterministically, and driving P2.
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
//!
//! # Where the numbers come from
//!
//! They were read off screenshots, not guessed. `examples/probe-boot.rs` runs
//! the machine and dumps frames, and every constant below was set by looking at
//! the screen the script actually lands on. Re-run it after touching them:
//!
//! ```text
//! just probe-boot rom=... game=lastblade2
//! ```

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

/// A game this lab knows how to boot and drive.
///
/// The rollback engine does not care which of these is running -- it only needs
/// `save_state`/`load_state`/`advance_frame`. What differs per game is the menu
/// choreography to get from a cold reset into a match, which is exactly the
/// part that cannot be made generic.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Game {
    /// Street Fighter Alpha 3, Capcom CPS-2.
    Sfa3,
    /// The Last Blade 2, SNK Neo Geo MVS.
    LastBlade2,
}

impl Game {
    pub const ALL: [Game; 2] = [Game::Sfa3, Game::LastBlade2];

    pub const fn as_str(self) -> &'static str {
        match self {
            Game::Sfa3 => "sfa3",
            Game::LastBlade2 => "lastblade2",
        }
    }

    /// The romset name FBNeo expects, i.e. the zip's basename.
    pub const fn romset(self) -> &'static str {
        match self {
            Game::Sfa3 => "sfa3",
            Game::LastBlade2 => "lastbld2",
        }
    }

    /// Whether the game needs `neogeo.zip`, the Neo Geo BIOS, alongside it.
    ///
    /// Neo Geo drivers are declared `STDROMPICKEXT(game, game, neogeo)`, which
    /// appends the BIOS romset to the list of files FBNeo requires. Four of its
    /// entries are not `BRF_OPT` and so are mandatory: `sp-s3.sp1`, `sm1.sm1`,
    /// `sfix.sfix` and `000-lo.lo`.
    pub const fn needs_bios(self) -> bool {
        match self {
            Game::Sfa3 => false,
            Game::LastBlade2 => true,
        }
    }

    /// Nominal refresh rate of the emulated hardware, for documentation and for
    /// reporting how far the session's tick rate is from native.
    pub const fn native_hz(self) -> f64 {
        match self {
            // CPS-2.
            Game::Sfa3 => 59.629_0,
            // Neo Geo MVS, as FBNeo reports it through `retro_get_system_av_info`.
            Game::LastBlade2 => 59.180_0,
        }
    }

    /// How many `Right` presses each side walks on the character-select grid.
    ///
    /// Named after the intent rather than the roster position: the point is
    /// that the two sides pick *different* slots, and that the slots are fixed
    /// in code rather than chosen at runtime, so both peers script the same
    /// menu inputs.
    pub const fn cursor_steps(self, player: usize) -> u32 {
        match (self, player) {
            (Game::Sfa3, 0) => 0,
            (Game::Sfa3, _) => 3,
            (Game::LastBlade2, 0) => 0,
            (Game::LastBlade2, _) => 2,
        }
    }

    /// Build the boot macro for one side.
    ///
    /// Both sides run their own copy at the same absolute frame numbers, so the
    /// two peers make identical menu inputs even though each only owns one
    /// player.
    pub fn boot_macro(self, player: usize) -> Macro {
        match self {
            Game::Sfa3 => sfa3_boot(self.cursor_steps(player)),
            Game::LastBlade2 => last_blade_2_boot(self.cursor_steps(player)),
        }
    }

    /// The move list the scripted opponent plays, from `player`'s side.
    pub fn repertoire(self, player: usize) -> Vec<Macro> {
        // Both games are 2D fighters with the same idea of forward and back,
        // so the move list is shared. The button *letters* differ between
        // CPS-2 and Neo Geo, but "the fastest normal" and "the parry" mean the
        // same thing to the experiment either way.
        repertoire(player)
    }
}

impl std::str::FromStr for Game {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "sfa3" => Ok(Game::Sfa3),
            "lastblade2" | "lastbld2" => Ok(Game::LastBlade2),
            other => Err(format!("unknown game '{other}' (expected sfa3|lastblade2)")),
        }
    }
}

impl std::fmt::Display for Game {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Frames to hold a menu button, and to wait afterwards.
const TAP: u32 = 6;
const SETTLE: u32 = 24;

// --- SFA3 (CPS-2) ----------------------------------------------------------

/// Frames to sit on the boot logos before touching anything.
const SFA3_BOOT_WAIT: u32 = 420; // 7 s
/// Frames to let the character-select intro and the round-start animation run.
const SFA3_SELECT_INTRO: u32 = 180; // 3 s
const SFA3_ROUND_INTRO: u32 = 300; // 5 s

fn sfa3_boot(cursor_steps: u32) -> Macro {
    let mut steps = vec![Step::wait(SFA3_BOOT_WAIT)];

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
    steps.push(Step::wait(SFA3_SELECT_INTRO));

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
    steps.push(Step::wait(SFA3_ROUND_INTRO));

    Macro::new(steps)
}

// --- The Last Blade 2 (Neo Geo MVS) ----------------------------------------
//
// The Neo Geo boot is longer than CPS-2 because the BIOS runs its own memory
// check and "MAX 330 MEGA PRO-GEAR SPEC" screen before the cartridge gets
// control. Every constant here was read off `probe-boot` screenshots.

/// Frames from reset to a machine that will accept a coin.
///
/// The BIOS memory check owns roughly the first 400 frames and ignores
/// everything; this waits past it and into the attract loop.
const LB2_BOOT_WAIT: u32 = 600; // 10 s
/// Frames to hold the coin switch, and to leave between coins.
///
/// Longer than a CPS-2 tap: the MVS coin input is debounced by the BIOS, and a
/// six-frame pulse is not reliably counted.
const LB2_COIN_HOLD: u32 = 12;
const LB2_COIN_GAP: u32 = 45;
/// When to press Start, and for how long.
///
/// The hold length is the surprise. A twelve-frame tap -- ample for the coin
/// switch, ample on CPS-2 -- does not start the game *at any frame*. This was
/// measured, not guessed: `examples/sweep-start.rs` tried a tap at every frame
/// from 640 to 2400, one freshly booted machine per candidate, covering four
/// full passes of the attract loop. Every one stayed in the demo.
///
/// Sweeping the hold length instead found the real rule. Starting at frame 700:
///
/// ```text
/// hold  45 frames -> attract demo
/// hold  70 frames -> attract demo
/// hold  75 frames -> Player Select     <-- both players in
/// hold 640 frames -> Player Select
/// ```
///
/// So the board wants Start *held* across a moment near frame 772, roughly 1.2
/// seconds, rather than pressed at a particular instant. 120 frames starting at
/// 700 covers 700..820, which is about 1.7x the measured minimum with slack on
/// both sides.
const LB2_START_AT: u32 = 700;
const LB2_START_HOLD: u32 = 120;
/// Last frame of the shortest hold that was observed to work: starting at 700,
/// 75 frames (700..=774) reached Player Select and 70 did not. Start must still
/// be held here -- `start_is_held_across_the_measured_accept_point` checks it.
#[cfg(test)]
const LB2_START_ACCEPT: u32 = 774;
/// Absolute frame at which the Player Select grid is interactive.
///
/// The grid appears around frame 850 with a fifteen-second countdown; this
/// leaves it a moment to settle.
const LB2_SELECT_AT: u32 = 900;
/// Frames to hold a direction on the grid, and to leave between steps.
const LB2_CURSOR_HOLD: u32 = 12;
const LB2_CURSOR_GAP: u32 = 18;
/// Frames to hold a menu confirmation.
///
/// Same rule as Start, and found the same way: this board wants menu buttons
/// *held*, not tapped. A six-frame tap on the character grid does nothing.
const LB2_CONFIRM_HOLD: u32 = 120;
/// The button that means "yes" on a Neo Geo menu.
///
/// This is Neo Geo button A ("weak slash"), which is what the Player Select and
/// Ability Select screens accept. Getting here took reading FBNeo's mapping:
/// under the classic pad layout the Neo Geo buttons come out *transposed*
/// relative to the obvious guess --
///
/// | Neo Geo | RetroPad | this lab's logical button |
/// |---------|----------|---------------------------|
/// | A       | B        | `Block`                   |
/// | B       | A        | `Confirm`                 |
/// | C       | Y        | `Attack`                  |
/// | D       | X        | `Special`                 |
///
/// so the button named `Confirm` here is the one thing that does *not* confirm.
/// The names are the arena's, and the arena is what named them; renaming them
/// to suit one emulated game would make the arena read strangely instead.
const LB2_CONFIRM_BUTTON: Button = Button::Block;
/// Frames between confirming the character and confirming the ability
/// ("Power" / "Speed") prompt that follows it.
const LB2_ABILITY_GAP: u32 = 60;
/// Frames from the ability prompt to a live round.
///
/// Measured: the ability confirm lands at frame 1260, and the machine then runs
/// the character portraits, the VS card, the stage title ("Battle Of Cloudy
/// Sky") and the round intro. The health bars and the round timer are up by
/// frame 1950. This hands over at 1980.
const LB2_ROUND_INTRO: u32 = 720; // 12 s

fn last_blade_2_boot(cursor_steps: u32) -> Macro {
    let mut steps = vec![Step::wait(LB2_BOOT_WAIT)];

    // MVS wants one credit per player. A twelve-frame hold is read as several
    // coin pulses, which is harmless -- the board just shows more credits.
    // The gap goes *between* the coins, not after the last one, so the phase
    // ends at a frame this file can state exactly.
    steps.push(Step::hold(LB2_COIN_HOLD, press(Button::Coin)));
    steps.push(Step::wait(LB2_COIN_GAP));
    steps.push(Step::hold(LB2_COIN_HOLD, press(Button::Coin)));

    // Idle until the frame the long Start hold begins.
    const COINS_END: u32 = LB2_BOOT_WAIT + LB2_COIN_HOLD * 2 + LB2_COIN_GAP;
    const _: () = assert!(
        COINS_END <= LB2_START_AT,
        "the coin phase must finish before the Start hold begins"
    );
    steps.push(Step::wait(LB2_START_AT - COINS_END));

    // One long hold, from both players at once. Both sides run this same macro
    // at the same absolute frames, so P1 Start and P2 Start are held together
    // and the board opens a two-player match directly.
    steps.push(Step::hold(LB2_START_HOLD, press(Button::Start)));

    // Idle until the grid is up. The cursor walk is sized so that even the
    // longer of the two scripts finishes well inside the select countdown.
    const START_END: u32 = LB2_START_AT + LB2_START_HOLD;
    const _: () = assert!(START_END <= LB2_SELECT_AT);
    steps.push(Step::wait(LB2_SELECT_AT - START_END));

    for _ in 0..cursor_steps {
        steps.push(Step::hold(LB2_CURSOR_HOLD, press(Button::Right)));
        steps.push(Step::wait(LB2_CURSOR_GAP));
    }

    // Confirm the character, then the "Power" / "Speed" ability prompt.
    steps.push(Step::hold(LB2_CONFIRM_HOLD, press(LB2_CONFIRM_BUTTON)));
    steps.push(Step::wait(LB2_ABILITY_GAP));
    steps.push(Step::hold(LB2_CONFIRM_HOLD, press(LB2_CONFIRM_BUTTON)));

    steps.push(Step::wait(LB2_ROUND_INTRO));

    Macro::new(steps)
}

/// Drives both players through the boot, then hands over to the real inputs.
pub struct BootDirector {
    game: Game,
    p1: Macro,
    p2: Macro,
    ready_at: u32,
}

impl BootDirector {
    pub fn new(game: Game) -> BootDirector {
        let p1 = game.boot_macro(0);
        let p2 = game.boot_macro(1);
        // The two macros differ in length by the cursor walk, so the match is
        // only "ready" once the longer of the two has finished.
        let ready_at = p1.total_frames().max(p2.total_frames());
        BootDirector {
            game,
            p1,
            p2,
            ready_at,
        }
    }

    pub fn game(&self) -> Game {
        self.game
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

/// The scripted opponent: a deterministic sequence of macros.
///
/// Like the arena bot, this is a *player*, not part of the simulation. It reads
/// nothing from the game -- it cannot, without ROM offsets -- so it plays a
/// fixed repertoire rather than reacting. That is enough to generate the input
/// churn a rollback experiment needs, which is what it is for.
pub struct ScriptedBot {
    rng: DeterministicRng,
    repertoire: Vec<Macro>,
    current: usize,
    elapsed: u32,
}

impl ScriptedBot {
    /// `player` is 0 for P1 and 1 for P2: it decides which way "forward" is,
    /// and it perturbs the seed so the two sides do not play the identical
    /// match in lockstep.
    ///
    /// Seeding per side is safe precisely because the bot is a *player*. Its
    /// randomness never has to agree between peers -- its outputs travel over
    /// the wire like any other input. It only has to be reproducible from the
    /// seed, so that `just bench` is repeatable.
    pub fn new(game: Game, seed: u64, player: usize) -> ScriptedBot {
        ScriptedBot {
            rng: DeterministicRng::new(
                seed ^ 0x5FA3_5FA3_5FA3_5FA3 ^ (player as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
            ),
            repertoire: game.repertoire(player),
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

/// The move list the scripted opponent plays, from `player`'s side of the
/// screen.
///
/// # Why this is a real move list and not a twitch generator
///
/// A bot that mashes random buttons produces input churn, which is enough to
/// make the rollback machinery work. It is not enough to make the *measurement*
/// mean anything, for two reasons.
///
/// First, prediction accuracy is a claim about how fighting-game inputs
/// actually behave: they are held for many frames at a time, which is why
/// "repeat the last confirmed input" guesses right ~93% of the time. A bot that
/// changes input every frame would make that number look far worse than a human
/// ever would; a bot that blocks for a second at a stretch and then throws a
/// four-step motion input exercises both extremes honestly.
///
/// Second, the frames that matter are the ones where a rollback is *visible* --
/// mid-combo, mid-motion, on wake-up. Those only exist if the bot performs
/// combos and motions.
///
/// # Side awareness
///
/// "Forward" is toward the opponent, which is Right for P1 and Left for P2. A
/// quarter-circle-forward is a different button sequence on each side, and a
/// bot that ignored this would throw its specials backwards and hold its guard
/// the wrong way -- turning every "block" into "walk into the attack".
///
/// # The buttons
///
/// Neo Geo, via the transposed mapping documented at [`LB2_CONFIRM_BUTTON`]:
/// A = weak slash (`Block`), B = strong slash (`Confirm`), C = kick
/// (`Attack`), D = repel (`Special`), C+D = throw.
fn repertoire(player: usize) -> Vec<Macro> {
    let neutral = PlayerInput::NEUTRAL;

    // Toward the opponent, and away from them.
    let (fwd_btn, back_btn) = if player == 0 {
        (Button::Right, Button::Left)
    } else {
        (Button::Left, Button::Right)
    };
    let fwd = press(fwd_btn);
    let back = press(back_btn);
    let down = press(Button::Down);
    let up = press(Button::Up);
    let down_fwd = down.with(fwd_btn);
    let down_back = down.with(back_btn);
    let up_fwd = up.with(fwd_btn);

    // Neo Geo face buttons, by what they do rather than by their letter.
    let weak = Button::Block;
    let strong = Button::Confirm;
    let kick = Button::Attack;
    let repel = Button::Special;

    vec![
        // --- offence -------------------------------------------------------
        // Walk in and poke with the fastest button.
        Macro::new(vec![
            Step::hold(24, fwd),
            Step::hold(4, press(weak)),
            Step::wait(14),
        ]),
        // Chain combo: weak -> strong -> kick, the bread-and-butter of the
        // system. Short gaps, because the links are tight.
        Macro::new(vec![
            Step::hold(3, press(weak)),
            Step::wait(2),
            Step::hold(3, press(strong)),
            Step::wait(2),
            Step::hold(4, press(kick)),
            Step::wait(22),
        ]),
        // Crouching sweep, then recover.
        Macro::new(vec![
            Step::hold(8, down),
            Step::hold(4, down.with(strong)),
            Step::wait(24),
        ]),
        // Jump in with an attack: the classic way to open someone up, and the
        // move most likely to have a rollback land in the middle of it.
        Macro::new(vec![
            Step::hold(4, up_fwd),
            Step::wait(22),
            Step::hold(4, press(kick)),
            Step::wait(20),
        ]),
        // --- motion inputs -------------------------------------------------
        // Quarter-circle forward + weak. Four distinct directions in twelve
        // frames: the hardest thing in the repertoire for a predictor to guess.
        Macro::new(vec![
            Step::hold(3, down),
            Step::hold(3, down_fwd),
            Step::hold(3, fwd),
            Step::hold(4, fwd.with(weak)),
            Step::wait(28),
        ]),
        // Quarter-circle back + strong.
        Macro::new(vec![
            Step::hold(3, down),
            Step::hold(3, down_back),
            Step::hold(3, back),
            Step::hold(4, back.with(strong)),
            Step::wait(28),
        ]),
        // Dragon-punch motion: forward, down, down-forward + strong.
        Macro::new(vec![
            Step::hold(3, fwd),
            Step::hold(3, down),
            Step::hold(3, down_fwd),
            Step::hold(4, down_fwd.with(strong)),
            Step::wait(30),
        ]),
        // --- defence -------------------------------------------------------
        // Stand guard. Holding away *is* blocking, and holding it for a second
        // is exactly the long-lived input that makes prediction work.
        Macro::new(vec![Step::hold(56, back)]),
        // Crouch guard, against lows.
        Macro::new(vec![Step::hold(48, down_back)]),
        // Repel: The Last Blade's parry. A four-frame window that either wins
        // the exchange outright or loses it.
        Macro::new(vec![Step::hold(6, press(repel)), Step::wait(26)]),
        // Low repel.
        Macro::new(vec![Step::hold(6, down.with(repel)), Step::wait(26)]),
        // Back off out of range.
        Macro::new(vec![
            Step::hold(3, back),
            Step::wait(2),
            Step::hold(3, back),
            Step::wait(18),
        ]),
        // --- close range ---------------------------------------------------
        // Throw: walk into them and press kick + repel together.
        Macro::new(vec![
            Step::hold(10, fwd),
            Step::hold(4, press(kick).with(repel)),
            Step::wait(24),
        ]),
        // --- neutral -------------------------------------------------------
        // Standing still is a real thing players do, and a bot that never stops
        // moving makes prediction look harder than it is.
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
    fn every_boot_script_presses_coin_before_start() {
        for game in Game::ALL {
            let m = game.boot_macro(0);
            let first_coin = (0..m.total_frames())
                .find(|&f| m.at(f).unwrap().contains(Button::Coin))
                .unwrap_or_else(|| panic!("{game} must insert a coin"));
            let first_start = (0..m.total_frames())
                .find(|&f| m.at(f).unwrap().contains(Button::Start))
                .unwrap_or_else(|| panic!("{game} must press start"));
            assert!(first_coin < first_start, "{game}: coin before start");
        }
    }

    #[test]
    fn every_boot_script_lets_the_logos_finish_first() {
        // Pressing during the BIOS check or the logo reel does nothing, and a
        // script that starts early ends up one screen behind for the whole run.
        for game in Game::ALL {
            let m = game.boot_macro(0);
            let first_press = (0..m.total_frames())
                .find(|&f| m.at(f).unwrap() != PlayerInput::NEUTRAL)
                .unwrap_or_else(|| panic!("{game} must press something"));
            let floor = match game {
                Game::Sfa3 => SFA3_BOOT_WAIT,
                Game::LastBlade2 => LB2_BOOT_WAIT,
            };
            assert_eq!(first_press, floor, "{game} touched the machine early");
        }
    }

    #[test]
    fn a_longer_cursor_walk_takes_more_frames() {
        for game in Game::ALL {
            assert!(
                game.boot_macro(1).total_frames() > game.boot_macro(0).total_frames(),
                "{game}: P2 walks further, so its script is longer"
            );
        }
    }

    #[test]
    fn start_is_held_across_the_measured_accept_point() {
        // The Last Blade 2 board acts on Start around frame 772, and only if it
        // has been held for roughly 75 frames by then -- a tap does nothing at
        // any frame. Both numbers came from `sweep-start`; if this fails,
        // re-measure with that tool rather than widening the assertion.
        for player in 0..2 {
            let m = Game::LastBlade2.boot_macro(player);
            let held: Vec<u32> = (0..m.total_frames())
                .filter(|&f| m.at(f).unwrap().contains(Button::Start))
                .collect();
            let first = *held.first().expect("must press start");
            let last = *held.last().unwrap();

            assert!(
                held.contains(&LB2_START_ACCEPT),
                "{player}: start not held at the accept point {LB2_START_ACCEPT}"
            );
            assert!(
                LB2_START_ACCEPT + 1 - first >= 75,
                "{player}: only {} frames of hold up to the accept point; \
                 75 was the measured minimum",
                LB2_START_ACCEPT + 1 - first
            );
            // Contiguous: a gap would release the button mid-hold.
            assert_eq!(
                held.len() as u32,
                last - first + 1,
                "{player}: the start hold is not contiguous"
            );
        }
    }

    #[test]
    fn a_game_round_trips_through_its_name() {
        for game in Game::ALL {
            assert_eq!(game.as_str().parse::<Game>(), Ok(game));
        }
        assert_eq!("lastbld2".parse::<Game>(), Ok(Game::LastBlade2));
        assert!("sf2".parse::<Game>().is_err());
    }

    #[test]
    fn only_the_neo_geo_game_needs_a_bios() {
        assert!(!Game::Sfa3.needs_bios());
        assert!(Game::LastBlade2.needs_bios());
    }

    #[test]
    fn the_director_hands_over_only_after_both_scripts_finish() {
        for game in Game::ALL {
            let d = BootDirector::new(game);
            assert_eq!(d.ready_at(), game.boot_macro(1).total_frames());
            assert!(!d.is_ready(d.ready_at() - 1));
            assert!(d.is_ready(d.ready_at()));

            assert!(d.input(0, 0).is_some());
            assert!(d.input(0, 1).is_some());
            assert_eq!(d.input(d.ready_at(), 0), None, "P1 goes to the human");
            assert_eq!(d.input(d.ready_at(), 1), None, "P2 goes to the bot");
        }
    }

    #[test]
    fn the_two_sides_select_different_characters() {
        for game in Game::ALL {
            let d = BootDirector::new(game);
            let rights = |player: usize| {
                (0..d.ready_at())
                    .filter(|&f| {
                        d.input(f, player)
                            .is_some_and(|i| i.contains(Button::Right))
                    })
                    .count()
            };
            assert_ne!(rights(0), rights(1), "{game}: cursors must end up apart");
        }
    }

    #[test]
    fn every_boot_fits_in_a_reasonable_wait() {
        // Every stage waits longer than the animation needs, so the boot is
        // slow on purpose -- but it still has to be short enough that `just
        // play` is usable, and short enough to fit inside a 180-second session
        // with plenty of match left over.
        for game in Game::ALL {
            let ready_at = BootDirector::new(game).ready_at();
            assert!(ready_at < 60 * 40, "{game}: {ready_at} frames is too long");
            assert!(ready_at > 60 * 10, "{game}: {ready_at} frames rushes it");
        }
    }

    #[test]
    fn the_bot_is_reproducible_from_its_seed() {
        let run = |seed| {
            let mut bot = ScriptedBot::new(Game::Sfa3, seed, 1);
            (0..3_000).map(|_| bot.decide()).collect::<Vec<_>>()
        };
        assert_eq!(run(11), run(11));
        assert_ne!(run(11), run(12));
    }

    #[test]
    fn the_bot_plays_a_whole_fighting_game_move_list() {
        let mut bot = ScriptedBot::new(Game::LastBlade2, 5, 1);
        let inputs: Vec<PlayerInput> = (0..20_000).map(|_| bot.decide()).collect();

        // Every button the game has, including the parry and the throw.
        for button in [
            Button::Block,
            Button::Confirm,
            Button::Attack,
            Button::Special,
        ] {
            assert!(
                inputs.iter().any(|i| i.contains(button)),
                "the bot never pressed {button:?}"
            );
        }
        // A throw is two buttons at once.
        assert!(
            inputs
                .iter()
                .any(|i| i.contains(Button::Attack) && i.contains(Button::Special)),
            "the bot never threw"
        );
        // Guarding: holding away for a long stretch.
        let longest_back = longest_run(&inputs, Button::Right);
        assert!(
            longest_back >= 40,
            "P2 guards by holding Right; longest hold was only {longest_back} frames"
        );
        // Motion inputs: a diagonal only happens during a quarter-circle or a
        // dragon punch, never in a plain walk.
        assert!(
            inputs
                .iter()
                .any(|i| i.contains(Button::Down) && i.contains(Button::Left)),
            "the bot never performed a motion input"
        );
        assert!(
            inputs.contains(&PlayerInput::NEUTRAL),
            "the bot never stood still"
        );
    }

    #[test]
    fn the_two_sides_face_each_other() {
        // Forward is toward the opponent, so P1 walks Right and P2 walks Left.
        // A bot that got this backwards would guard into the attack and throw
        // its specials the wrong way -- and the measurements would be of a
        // fight that never happened.
        let run = |player| {
            let mut bot = ScriptedBot::new(Game::LastBlade2, 3, player);
            (0..20_000).map(|_| bot.decide()).collect::<Vec<_>>()
        };
        let p1 = run(0);
        let p2 = run(1);
        assert!(longest_run(&p1, Button::Left) > longest_run(&p1, Button::Right));
        assert!(longest_run(&p2, Button::Right) > longest_run(&p2, Button::Left));
    }

    /// Longest streak of consecutive frames holding `button`.
    fn longest_run(inputs: &[PlayerInput], button: Button) -> usize {
        let mut best = 0;
        let mut run = 0;
        for i in inputs {
            if i.contains(button) {
                run += 1;
                best = best.max(run);
            } else {
                run = 0;
            }
        }
        best
    }

    #[test]
    fn the_bot_never_holds_opposing_directions() {
        let mut bot = ScriptedBot::new(Game::LastBlade2, 9, 1);
        for _ in 0..10_000 {
            let i = bot.decide();
            let horizontal = i.contains(Button::Left) && i.contains(Button::Right);
            assert!(!horizontal, "left and right held together");
            let vertical = i.contains(Button::Up) && i.contains(Button::Down);
            assert!(!vertical, "up and down held together");
        }
    }
}
