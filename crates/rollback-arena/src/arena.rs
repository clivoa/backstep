//! The deterministic 2D arena.
//!
//! Two fighters on a flat stage: walk, jump, attack, block, a projectile, hit
//! stun and health. It exists to make rollback *visible* -- every quantity that
//! matters is an integer, the state blob is 204 bytes, and the checksum covers
//! all of it, so a single wrong frame is caught by the next checksum exchange.
//!
//! Determinism rules obeyed here, all of which the emulated path gets for free from
//! the emulator but which have to be enforced by hand in native code:
//!
//! * no floating point (see [`crate::fixed`]);
//! * no hashing, iteration over hash maps, or pointer-derived values;
//! * no wall-clock time, no thread scheduling, no randomness in the simulation;
//! * every loop runs a fixed number of iterations in a fixed order.

use rollback_core::{Button, OutputMode, PlayerInput, Simulation, SimulationError};

use crate::codec::{Reader, Writer};
use crate::fixed::{self, from_px};

/// Slots in the projectile pool. Fixed size: a growable list would make the
/// state blob variable-length for no gameplay benefit.
pub const MAX_PROJECTILES: usize = 4;
/// Exact size of a saved state, in bytes.
pub const STATE_BYTES: usize = 4 + 2 * FIGHTER_WORDS * 4 + MAX_PROJECTILES * 6 * 4 + 6 * 4;
const FIGHTER_WORDS: usize = 10;

pub const STAGE_MIN_X: i32 = from_px(20);
pub const STAGE_MAX_X: i32 = from_px(380);
pub const FIGHTER_HALF_WIDTH: i32 = from_px(14);
pub const MAX_HEALTH: i32 = 1000;

const WALK_SPEED: i32 = 384; // 1.5 px/frame
const AIR_DRIFT: i32 = 192; // 0.75 px/frame
const JUMP_SPEED: i32 = from_px(7);
const GRAVITY: i32 = 96; // 0.375 px/frame^2

const ATTACK_STARTUP: u32 = 4;
const ATTACK_ACTIVE: u32 = 3;
const ATTACK_TOTAL: u32 = 16;
const ATTACK_REACH: i32 = from_px(46);
const ATTACK_HEIGHT: i32 = from_px(40);
const ATTACK_DAMAGE: i32 = 70;
const CHIP_DAMAGE: i32 = 12;
const HITSTUN_FRAMES: u32 = 18;
const BLOCKSTUN_FRAMES: u32 = 9;
const HIT_PUSHBACK: i32 = from_px(7);

const SPECIAL_TOTAL: u32 = 22;
const SPECIAL_RELEASE: u32 = 6;
const SPECIAL_COOLDOWN: u32 = 50;
const PROJECTILE_SPEED: i32 = from_px(6);
const PROJECTILE_DAMAGE: i32 = 45;
const PROJECTILE_HALF_WIDTH: i32 = from_px(8);
const PROJECTILE_TTL: u32 = 100;

const ROUND_FRAMES: u32 = 60 * 60;
const KO_FREEZE_FRAMES: u32 = 90;

const START_X: [i32; 2] = [from_px(140), from_px(260)];

/// What a fighter is currently committed to.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u32)]
pub enum Action {
    #[default]
    Idle = 0,
    Walk = 1,
    Airborne = 2,
    Attack = 3,
    Special = 4,
    Block = 5,
    Hitstun = 6,
    Blockstun = 7,
    Ko = 8,
}

impl Action {
    fn from_u32(v: u32) -> Action {
        match v {
            1 => Action::Walk,
            2 => Action::Airborne,
            3 => Action::Attack,
            4 => Action::Special,
            5 => Action::Block,
            6 => Action::Hitstun,
            7 => Action::Blockstun,
            8 => Action::Ko,
            _ => Action::Idle,
        }
    }

    /// True while the fighter cannot start a new action.
    fn is_committed(self) -> bool {
        matches!(
            self,
            Action::Attack | Action::Special | Action::Hitstun | Action::Blockstun | Action::Ko
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Fighter {
    pub x: i32,
    pub y: i32,
    pub vy: i32,
    /// +1 facing right, -1 facing left.
    pub facing: i32,
    pub health: i32,
    pub action: Action,
    /// Frames spent in the current action.
    pub action_timer: u32,
    pub special_cooldown: u32,
    /// True once the current attack has already connected, so one swing cannot
    /// hit twice.
    pub attack_connected: bool,
    pub combo: u32,
}

impl Fighter {
    fn new(index: usize) -> Fighter {
        Fighter {
            x: START_X[index],
            y: 0,
            vy: 0,
            facing: if index == 0 { 1 } else { -1 },
            health: MAX_HEALTH,
            action: Action::Idle,
            action_timer: 0,
            special_cooldown: 0,
            attack_connected: false,
            combo: 0,
        }
    }

    pub fn on_ground(&self) -> bool {
        self.y <= 0
    }

    pub fn is_ko(&self) -> bool {
        self.health <= 0
    }

    /// Frames the current attack has been active for, if it is active now.
    fn attack_is_active(&self) -> bool {
        self.action == Action::Attack
            && self.action_timer >= ATTACK_STARTUP
            && self.action_timer < ATTACK_STARTUP + ATTACK_ACTIVE
            && !self.attack_connected
    }

    fn is_blocking_toward(&self, attacker_x: i32) -> bool {
        self.action == Action::Block && fixed::signum(attacker_x - self.x) == self.facing
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Projectile {
    pub active: bool,
    pub x: i32,
    pub y: i32,
    pub vx: i32,
    pub owner: u32,
    pub ttl: u32,
}

/// The complete simulation state, plus one field that is deliberately outside it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Arena {
    pub frame: u32,
    pub fighters: [Fighter; 2],
    pub projectiles: [Projectile; MAX_PROJECTILES],
    next_projectile: u32,
    pub rounds_won: [u32; 2],
    pub round_timer: u32,
    /// Countdown after a KO before the next round starts.
    pub freeze: u32,

    /// Frames advanced in `Present` mode. Not saved, not checksummed: it counts
    /// *display* frames, which legitimately differ between the two peers, and
    /// including it would turn every rollback into a false desync.
    pub presented_frames: u64,
}

impl Default for Arena {
    fn default() -> Self {
        Arena::new()
    }
}

impl Arena {
    pub fn new() -> Arena {
        Arena {
            frame: 0,
            fighters: [Fighter::new(0), Fighter::new(1)],
            projectiles: [Projectile::default(); MAX_PROJECTILES],
            next_projectile: 0,
            rounds_won: [0, 0],
            round_timer: ROUND_FRAMES,
            freeze: 0,
            presented_frames: 0,
        }
    }

    /// Horizontal gap between the fighters, in fixed point.
    pub fn gap(&self) -> i32 {
        fixed::abs(self.fighters[1].x - self.fighters[0].x)
    }

    /// Any live projectile heading toward fighter `index`.
    pub fn incoming_projectile(&self, index: usize) -> Option<&Projectile> {
        self.projectiles.iter().find(|p| {
            p.active
                && p.owner != index as u32
                && fixed::signum(self.fighters[index].x - p.x) == fixed::signum(p.vx)
        })
    }

    fn reset_round(&mut self) {
        self.fighters = [Fighter::new(0), Fighter::new(1)];
        self.projectiles = [Projectile::default(); MAX_PROJECTILES];
        self.next_projectile = 0;
        self.round_timer = ROUND_FRAMES;
    }

    fn step(&mut self, inputs: [PlayerInput; 2]) {
        self.frame = self.frame.wrapping_add(1);

        // A KO freezes everything: no input, no physics, no timer. When it
        // lapses the next round starts from the neutral position.
        if self.freeze > 0 {
            self.freeze -= 1;
            if self.freeze == 0 {
                self.reset_round();
            }
            return;
        }

        self.round_timer = self.round_timer.saturating_sub(1);

        for i in 0..2 {
            self.tick_timers(i);
        }
        for (i, input) in inputs.into_iter().enumerate() {
            self.apply_input(i, input);
        }
        for i in 0..2 {
            self.integrate(i);
        }
        self.separate();
        for i in 0..2 {
            self.resolve_melee(i);
        }
        self.step_projectiles();
        self.check_round_end();
    }

    fn tick_timers(&mut self, i: usize) {
        let f = &mut self.fighters[i];
        f.special_cooldown = f.special_cooldown.saturating_sub(1);

        if f.is_ko() {
            f.action = Action::Ko;
            return;
        }

        f.action_timer = f.action_timer.saturating_add(1);
        let expired = match f.action {
            Action::Attack => f.action_timer >= ATTACK_TOTAL,
            Action::Special => f.action_timer >= SPECIAL_TOTAL,
            Action::Hitstun => f.action_timer >= HITSTUN_FRAMES,
            Action::Blockstun => f.action_timer >= BLOCKSTUN_FRAMES,
            _ => false,
        };
        if expired {
            f.action = Action::Idle;
            f.action_timer = 0;
            f.attack_connected = false;
            f.combo = 0;
        }
    }

    fn apply_input(&mut self, i: usize, input: PlayerInput) {
        let opponent_x = self.fighters[1 - i].x;
        let f = &mut self.fighters[i];

        if f.is_ko() {
            return;
        }

        // Turn to face the opponent, but only while free to act: a committed
        // attack keeps the direction it started with.
        if f.on_ground() && !f.action.is_committed() {
            let toward = fixed::signum(opponent_x - f.x);
            if toward != 0 {
                f.facing = toward;
            }
        }

        // Release the projectile mid-`Special`, then keep the recovery frames.
        if f.action == Action::Special {
            if f.action_timer == SPECIAL_RELEASE {
                let (x, y, vx, owner) = (f.x, f.y, f.facing * PROJECTILE_SPEED, i as u32);
                self.spawn_projectile(x, y, vx, owner);
            }
            return;
        }
        if f.action.is_committed() {
            return;
        }

        let grounded = f.on_ground();

        if grounded && input.contains(Button::Attack) {
            f.action = Action::Attack;
            f.action_timer = 0;
            f.attack_connected = false;
            return;
        }
        if grounded && input.contains(Button::Special) && f.special_cooldown == 0 {
            f.action = Action::Special;
            f.action_timer = 0;
            f.special_cooldown = SPECIAL_COOLDOWN;
            return;
        }
        if grounded && input.contains(Button::Block) {
            f.action = Action::Block;
            return;
        }
        if grounded && input.contains(Button::Up) {
            f.vy = JUMP_SPEED;
            f.action = Action::Airborne;
        }

        // Horizontal movement. Left and Right pressed together cancel out,
        // which is what a real stick gate would enforce anyway.
        let dir =
            i32::from(input.contains(Button::Right)) - i32::from(input.contains(Button::Left));
        if dir != 0 {
            let speed = if grounded { WALK_SPEED } else { AIR_DRIFT };
            f.x += dir * speed;
            if grounded && f.action != Action::Airborne {
                f.action = Action::Walk;
            }
        } else if grounded && f.action == Action::Walk {
            f.action = Action::Idle;
        }
        if grounded && f.action == Action::Block && !input.contains(Button::Block) {
            f.action = Action::Idle;
        }
    }

    fn integrate(&mut self, i: usize) {
        let f = &mut self.fighters[i];
        if !f.on_ground() || f.vy > 0 {
            f.y += f.vy;
            f.vy -= GRAVITY;
            if f.y <= 0 {
                f.y = 0;
                f.vy = 0;
                if f.action == Action::Airborne {
                    f.action = Action::Idle;
                }
            } else if f.action != Action::Attack && f.action != Action::Hitstun {
                f.action = Action::Airborne;
            }
        }
        f.x = fixed::clamp(f.x, STAGE_MIN_X, STAGE_MAX_X);
    }

    /// Keep the bodies from overlapping, splitting the correction evenly.
    ///
    /// The odd fixed-point unit goes to fighter 0 -- arbitrary, but fixed, so
    /// both peers make the same choice.
    fn separate(&mut self) {
        let min_gap = FIGHTER_HALF_WIDTH * 2;
        let delta = self.fighters[1].x - self.fighters[0].x;
        let gap = fixed::abs(delta);
        if gap >= min_gap {
            return;
        }
        let overlap = min_gap - gap;
        let dir = if delta == 0 { 1 } else { fixed::signum(delta) };
        let half = overlap / 2;
        self.fighters[0].x -= dir * (overlap - half);
        self.fighters[1].x += dir * half;
        self.fighters[0].x = fixed::clamp(self.fighters[0].x, STAGE_MIN_X, STAGE_MAX_X);
        self.fighters[1].x = fixed::clamp(self.fighters[1].x, STAGE_MIN_X, STAGE_MAX_X);
    }

    fn resolve_melee(&mut self, attacker: usize) {
        if !self.fighters[attacker].attack_is_active() {
            return;
        }
        let defender = 1 - attacker;
        let (ax, ay, facing) = {
            let a = &self.fighters[attacker];
            (a.x, a.y, a.facing)
        };
        let (dx, dy) = (self.fighters[defender].x, self.fighters[defender].y);

        let horizontal = dx - ax;
        let in_front = fixed::signum(horizontal) == facing || horizontal == 0;
        let in_range = fixed::abs(horizontal) <= ATTACK_REACH + FIGHTER_HALF_WIDTH;
        let same_height = fixed::abs(dy - ay) <= ATTACK_HEIGHT;
        if !(in_front && in_range && same_height) {
            return;
        }

        self.fighters[attacker].attack_connected = true;
        let blocked = self.fighters[defender].is_blocking_toward(ax);
        self.apply_damage(
            defender,
            if blocked { CHIP_DAMAGE } else { ATTACK_DAMAGE },
            blocked,
            facing,
        );
    }

    fn step_projectiles(&mut self) {
        for slot in 0..MAX_PROJECTILES {
            if !self.projectiles[slot].active {
                continue;
            }
            let (x, vx, owner) = {
                let p = &mut self.projectiles[slot];
                p.x += p.vx;
                p.ttl = p.ttl.saturating_sub(1);
                if p.ttl == 0 || p.x < STAGE_MIN_X || p.x > STAGE_MAX_X {
                    p.active = false;
                    continue;
                }
                (p.x, p.vx, p.owner)
            };

            let target = 1 - owner as usize;
            let t = &self.fighters[target];
            let hit = fixed::abs(t.x - x) <= FIGHTER_HALF_WIDTH + PROJECTILE_HALF_WIDTH
                && fixed::abs(t.y - self.projectiles[slot].y) <= ATTACK_HEIGHT;
            if !hit {
                continue;
            }

            self.projectiles[slot].active = false;
            let blocked = self.fighters[target].is_blocking_toward(x);
            let damage = if blocked {
                CHIP_DAMAGE
            } else {
                PROJECTILE_DAMAGE
            };
            self.apply_damage(target, damage, blocked, fixed::signum(vx));
        }
    }

    fn apply_damage(&mut self, target: usize, damage: i32, blocked: bool, push_dir: i32) {
        let f = &mut self.fighters[target];
        f.health = (f.health - damage).max(0);
        f.x = fixed::clamp(f.x + push_dir * HIT_PUSHBACK, STAGE_MIN_X, STAGE_MAX_X);
        f.action_timer = 0;
        if blocked {
            f.action = Action::Blockstun;
        } else {
            f.action = Action::Hitstun;
            f.combo = f.combo.saturating_add(1);
        }
        if f.health == 0 {
            f.action = Action::Ko;
        }
    }

    fn spawn_projectile(&mut self, x: i32, y: i32, vx: i32, owner: u32) {
        // Round-robin over the pool: with four slots and a 50-frame cooldown a
        // slot is always free, but overwriting the oldest is still the
        // deterministic fallback rather than dropping the shot silently.
        let slot = (self.next_projectile as usize) % MAX_PROJECTILES;
        self.next_projectile = (self.next_projectile + 1) % MAX_PROJECTILES as u32;
        self.projectiles[slot] = Projectile {
            active: true,
            x: x + fixed::signum(vx) * FIGHTER_HALF_WIDTH,
            y: y + from_px(20),
            vx,
            owner,
            ttl: PROJECTILE_TTL,
        };
    }

    fn check_round_end(&mut self) {
        if self.freeze > 0 {
            return;
        }
        let ko = self.fighters[0].is_ko() || self.fighters[1].is_ko();
        let timeout = self.round_timer == 0;
        if !ko && !timeout {
            return;
        }

        let (h0, h1) = (self.fighters[0].health, self.fighters[1].health);
        if h0 > h1 {
            self.rounds_won[0] += 1;
        } else if h1 > h0 {
            self.rounds_won[1] += 1;
        }
        self.freeze = KO_FREEZE_FRAMES;
    }
}

impl Simulation for Arena {
    fn save_state(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(STATE_BYTES);
        w.u32(self.frame);
        for f in &self.fighters {
            w.i32(f.x);
            w.i32(f.y);
            w.i32(f.vy);
            w.i32(f.facing);
            w.i32(f.health);
            w.u32(f.action as u32);
            w.u32(f.action_timer);
            w.u32(f.special_cooldown);
            w.bool(f.attack_connected);
            w.u32(f.combo);
        }
        for p in &self.projectiles {
            w.bool(p.active);
            w.i32(p.x);
            w.i32(p.y);
            w.i32(p.vx);
            w.u32(p.owner);
            w.u32(p.ttl);
        }
        w.u32(self.next_projectile);
        w.u32(self.rounds_won[0]);
        w.u32(self.rounds_won[1]);
        w.u32(self.round_timer);
        w.u32(self.freeze);
        w.u32(0); // reserved, keeps STATE_BYTES a round multiple of the layout
        w.finish()
    }

    fn load_state(&mut self, data: &[u8]) -> Result<(), SimulationError> {
        let mut r = Reader::new(data, STATE_BYTES)?;
        self.frame = r.u32();
        for f in &mut self.fighters {
            f.x = r.i32();
            f.y = r.i32();
            f.vy = r.i32();
            f.facing = r.i32();
            f.health = r.i32();
            f.action = Action::from_u32(r.u32());
            f.action_timer = r.u32();
            f.special_cooldown = r.u32();
            f.attack_connected = r.bool();
            f.combo = r.u32();
        }
        for p in &mut self.projectiles {
            p.active = r.bool();
            p.x = r.i32();
            p.y = r.i32();
            p.vx = r.i32();
            p.owner = r.u32();
            p.ttl = r.u32();
        }
        self.next_projectile = r.u32();
        self.rounds_won = [r.u32(), r.u32()];
        self.round_timer = r.u32();
        self.freeze = r.u32();
        let _reserved = r.u32();
        // `presented_frames` is intentionally not restored: it counts what this
        // peer displayed, which a rollback does not undo.
        Ok(())
    }

    fn advance_frame(&mut self, inputs: [PlayerInput; 2], output_mode: OutputMode) {
        self.step(inputs);
        if output_mode.emits_output() {
            self.presented_frames += 1;
        }
    }

    fn checksum(&self) -> u64 {
        let mut h = rollback_core::Fnv1a::new();
        h.write(&self.save_state());
        h.finish()
    }
}

/// Pixel-space view of the arena, for the SDL2 renderer and the overlay.
pub struct FighterView {
    pub x_px: i32,
    pub y_px: i32,
    pub facing: i32,
    pub health: i32,
    pub action: Action,
}

impl Arena {
    pub fn fighter_view(&self, index: usize) -> FighterView {
        let f = &self.fighters[index];
        FighterView {
            x_px: fixed::to_px(f.x),
            y_px: fixed::to_px(f.y),
            facing: f.facing,
            health: f.health,
            action: f.action,
        }
    }

    /// Health as a percentage, for the HUD.
    pub fn health_percent(&self, index: usize) -> i32 {
        self.fighters[index].health * 100 / MAX_HEALTH
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rollback_core::Button;

    fn press(buttons: &[Button]) -> PlayerInput {
        buttons
            .iter()
            .fold(PlayerInput::NEUTRAL, |acc, &b| acc.with(b))
    }

    fn run(arena: &mut Arena, frames: usize, p1: PlayerInput, p2: PlayerInput) {
        for _ in 0..frames {
            arena.advance_frame([p1, p2], OutputMode::Present);
        }
    }

    #[test]
    fn state_blob_has_the_documented_size() {
        assert_eq!(Arena::new().save_state().len(), STATE_BYTES);
        assert_eq!(STATE_BYTES, 204);
    }

    #[test]
    fn state_round_trips_exactly() {
        let mut a = Arena::new();
        run(
            &mut a,
            120,
            press(&[Button::Right, Button::Attack]),
            press(&[Button::Special]),
        );
        let blob = a.save_state();

        let mut b = Arena::new();
        b.load_state(&blob).unwrap();
        assert_eq!(a.checksum(), b.checksum());
        assert_eq!(a.save_state(), b.save_state());
    }

    #[test]
    fn fighters_start_apart_and_face_each_other() {
        let a = Arena::new();
        assert_eq!(a.fighters[0].facing, 1);
        assert_eq!(a.fighters[1].facing, -1);
        assert!(a.gap() > FIGHTER_HALF_WIDTH * 2);
    }

    #[test]
    fn walking_moves_and_the_stage_has_walls() {
        let mut a = Arena::new();
        let start = a.fighters[0].x;
        run(&mut a, 10, press(&[Button::Right]), PlayerInput::NEUTRAL);
        assert!(a.fighters[0].x > start);

        run(&mut a, 600, press(&[Button::Left]), PlayerInput::NEUTRAL);
        assert_eq!(a.fighters[0].x, STAGE_MIN_X, "must stop at the wall");
    }

    #[test]
    fn fighters_never_overlap() {
        let mut a = Arena::new();
        for _ in 0..600 {
            a.advance_frame(
                [press(&[Button::Right]), press(&[Button::Left])],
                OutputMode::Present,
            );
            assert!(
                a.gap() >= FIGHTER_HALF_WIDTH * 2,
                "bodies overlapped at frame {}",
                a.frame
            );
        }
    }

    #[test]
    fn a_jump_leaves_the_ground_and_comes_back() {
        let mut a = Arena::new();
        run(&mut a, 1, press(&[Button::Up]), PlayerInput::NEUTRAL);
        run(&mut a, 8, PlayerInput::NEUTRAL, PlayerInput::NEUTRAL);
        assert!(a.fighters[0].y > 0, "should be airborne");
        run(&mut a, 60, PlayerInput::NEUTRAL, PlayerInput::NEUTRAL);
        assert_eq!(a.fighters[0].y, 0, "gravity must bring it back down");
        assert!(a.fighters[0].on_ground());
    }

    #[test]
    fn an_attack_in_range_takes_health() {
        let mut a = Arena::new();
        // Close the distance first, then swing.
        run(&mut a, 40, press(&[Button::Right]), press(&[Button::Left]));
        let before = a.fighters[1].health;
        run(
            &mut a,
            ATTACK_TOTAL as usize,
            press(&[Button::Attack]),
            PlayerInput::NEUTRAL,
        );
        assert_eq!(a.fighters[1].health, before - ATTACK_DAMAGE);
        assert_eq!(a.fighters[1].action, Action::Hitstun);
    }

    #[test]
    fn one_swing_can_only_connect_once() {
        let mut a = Arena::new();
        run(&mut a, 40, press(&[Button::Right]), press(&[Button::Left]));
        let before = a.fighters[1].health;
        // Hold attack through several active frames.
        run(
            &mut a,
            ATTACK_TOTAL as usize - 1,
            press(&[Button::Attack]),
            PlayerInput::NEUTRAL,
        );
        assert_eq!(a.fighters[1].health, before - ATTACK_DAMAGE);
    }

    #[test]
    fn blocking_converts_a_hit_into_chip_damage() {
        let mut a = Arena::new();
        run(&mut a, 40, press(&[Button::Right]), press(&[Button::Left]));
        let before = a.fighters[1].health;
        run(
            &mut a,
            ATTACK_TOTAL as usize,
            press(&[Button::Attack]),
            press(&[Button::Block]),
        );
        assert_eq!(a.fighters[1].health, before - CHIP_DAMAGE);
    }

    #[test]
    fn an_attack_out_of_range_does_nothing() {
        let mut a = Arena::new();
        let before = a.fighters[1].health;
        run(
            &mut a,
            ATTACK_TOTAL as usize,
            press(&[Button::Attack]),
            PlayerInput::NEUTRAL,
        );
        assert_eq!(a.fighters[1].health, before);
    }

    #[test]
    fn a_projectile_crosses_the_stage_and_connects() {
        let mut a = Arena::new();
        let before = a.fighters[1].health;
        run(&mut a, 1, press(&[Button::Special]), PlayerInput::NEUTRAL);
        run(
            &mut a,
            SPECIAL_RELEASE as usize,
            PlayerInput::NEUTRAL,
            PlayerInput::NEUTRAL,
        );
        assert!(a.projectiles.iter().any(|p| p.active), "shot must exist");

        run(&mut a, 60, PlayerInput::NEUTRAL, PlayerInput::NEUTRAL);
        assert_eq!(a.fighters[1].health, before - PROJECTILE_DAMAGE);
    }

    #[test]
    fn the_special_respects_its_cooldown() {
        let mut a = Arena::new();
        run(
            &mut a,
            SPECIAL_TOTAL as usize + 1,
            press(&[Button::Special]),
            PlayerInput::NEUTRAL,
        );
        let live = a.projectiles.iter().filter(|p| p.active).count();
        // Cooldown outlasts the move, so holding the button cannot chain shots.
        run(&mut a, 10, press(&[Button::Special]), PlayerInput::NEUTRAL);
        assert_eq!(a.projectiles.iter().filter(|p| p.active).count(), live);
    }

    #[test]
    fn a_ko_freezes_the_round_and_then_resets_it() {
        let mut a = Arena::new();
        a.fighters[1].health = 1;
        run(&mut a, 40, press(&[Button::Right]), press(&[Button::Left]));
        run(
            &mut a,
            ATTACK_TOTAL as usize,
            press(&[Button::Attack]),
            PlayerInput::NEUTRAL,
        );
        assert!(a.freeze > 0, "a KO must freeze the round");
        assert_eq!(a.rounds_won[0], 1);

        run(
            &mut a,
            KO_FREEZE_FRAMES as usize + 1,
            PlayerInput::NEUTRAL,
            PlayerInput::NEUTRAL,
        );
        assert_eq!(a.fighters[1].health, MAX_HEALTH, "next round starts fresh");
        assert_eq!(a.rounds_won[0], 1, "the score survives the reset");
    }

    #[test]
    fn output_mode_cannot_touch_simulation_state() {
        let inputs = [
            press(&[Button::Right, Button::Attack]),
            press(&[Button::Block]),
        ];
        let mut shown = Arena::new();
        let mut replayed = Arena::new();
        for _ in 0..300 {
            shown.advance_frame(inputs, OutputMode::Present);
            replayed.advance_frame(inputs, OutputMode::Resimulate);
        }
        assert_eq!(shown.save_state(), replayed.save_state());
        assert_eq!(shown.presented_frames, 300);
        assert_eq!(replayed.presented_frames, 0);
    }

    #[test]
    fn the_same_inputs_always_produce_the_same_checksum() {
        let script: Vec<[PlayerInput; 2]> = (0..2000)
            .map(|f: u32| {
                [
                    PlayerInput((f.wrapping_mul(2654435761) >> 16) as u16 & 0x7F),
                    PlayerInput((f.wrapping_mul(40503) >> 8) as u16 & 0x7F),
                ]
            })
            .collect();

        let mut a = Arena::new();
        let mut b = Arena::new();
        for inputs in &script {
            a.advance_frame(*inputs, OutputMode::Present);
            b.advance_frame(*inputs, OutputMode::Resimulate);
        }
        assert_eq!(a.checksum(), b.checksum());
    }

    #[test]
    fn loading_an_undersized_blob_is_an_error() {
        let mut a = Arena::new();
        assert!(a.load_state(&[0u8; 8]).is_err());
    }

    #[test]
    fn health_percent_tracks_damage() {
        let mut a = Arena::new();
        assert_eq!(a.health_percent(0), 100);
        a.fighters[0].health = MAX_HEALTH / 4;
        assert_eq!(a.health_percent(0), 25);
    }
}
