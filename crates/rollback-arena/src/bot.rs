//! A reactive finite-state-machine opponent for the arena.
//!
//! The bot reads the arena state and produces one `PlayerInput` per frame, the
//! same way a human's controller does. That is the point: it is a *player*, not
//! part of the simulation. It runs only on the peer that owns it, its input
//! travels over the wire like any other, and the simulation never observes its
//! internals -- so its randomness cannot cause a desync.
//!
//! It is still seeded deterministically, because `just bench` has to be
//! repeatable: same seed, same profile, same match.

use rollback_core::{Button, DeterministicRng, PlayerInput};

use crate::arena::{Action, Arena, FIGHTER_HALF_WIDTH};
use crate::fixed::{self, from_px};

/// Distance at which the bot considers itself in striking range.
const STRIKE_RANGE: i32 = from_px(52);
/// Distance beyond which it prefers to throw a projectile.
const ZONE_RANGE: i32 = from_px(150);
/// How long a chosen plan is honoured before the bot reconsiders.
const PLAN_FRAMES: u32 = 12;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Plan {
    /// Walk toward the opponent.
    Approach,
    /// Back off and wait out the opponent's move.
    Retreat,
    /// Swing.
    Strike,
    /// Throw a projectile from range.
    Zone,
    /// Hold block.
    Guard,
    /// Jump in.
    Leap,
}

pub struct ArenaBot {
    rng: DeterministicRng,
    plan: Plan,
    plan_timer: u32,
    /// Which fighter this bot drives.
    index: usize,
    /// 0..=100. Higher means more willing to attack than to guard.
    aggression: u32,
}

impl ArenaBot {
    pub fn new(index: usize, seed: u64) -> Self {
        // Derive per-side seeds so P1's and P2's bots do not act in lockstep
        // when a bench run gives both the same session seed.
        let mut rng = DeterministicRng::new(seed ^ (index as u64).wrapping_mul(0x9E37_79B9));
        let aggression = 45 + rng.below(35);
        Self {
            rng,
            plan: Plan::Approach,
            plan_timer: 0,
            index,
            aggression,
        }
    }

    pub fn plan(&self) -> Plan {
        self.plan
    }

    /// Decide this frame's input. Call exactly once per *presented* frame.
    pub fn decide(&mut self, arena: &Arena) -> PlayerInput {
        let me = &arena.fighters[self.index];
        let them = &arena.fighters[1 - self.index];

        // Stunned or dead: nothing to send but neutral.
        if me.is_ko() || matches!(me.action, Action::Hitstun | Action::Ko) {
            return PlayerInput::NEUTRAL;
        }

        let gap = fixed::abs(them.x - me.x);
        let toward = fixed::signum(them.x - me.x);

        self.plan_timer = self.plan_timer.saturating_sub(1);
        if self.plan_timer == 0 {
            self.plan = self.choose_plan(arena, gap);
            self.plan_timer = PLAN_FRAMES;
        }

        // Reflexes override the plan: they are a reaction to this frame, not a
        // decision made twelve frames ago.
        if arena.incoming_projectile(self.index).is_some() && gap > STRIKE_RANGE {
            return PlayerInput::NEUTRAL.with(Button::Block);
        }
        if them.action == Action::Attack && gap <= STRIKE_RANGE + FIGHTER_HALF_WIDTH {
            return PlayerInput::NEUTRAL.with(Button::Block);
        }

        let mut input = PlayerInput::NEUTRAL;
        match self.plan {
            Plan::Approach => {
                input.set(direction(toward), true);
            }
            Plan::Retreat => {
                input.set(direction(-toward), true);
            }
            Plan::Strike => {
                if gap <= STRIKE_RANGE {
                    input.set(Button::Attack, true);
                } else {
                    input.set(direction(toward), true);
                }
            }
            Plan::Zone => {
                if me.special_cooldown == 0 {
                    input.set(Button::Special, true);
                } else {
                    input.set(direction(-toward), true);
                }
            }
            Plan::Guard => {
                input.set(Button::Block, true);
            }
            Plan::Leap => {
                input.set(Button::Up, true);
                input.set(direction(toward), true);
            }
        }
        input
    }

    fn choose_plan(&mut self, arena: &Arena, gap: i32) -> Plan {
        let me = &arena.fighters[self.index];
        let roll = self.rng.below(100);

        if gap <= STRIKE_RANGE {
            // Close range: trade between swinging and guarding.
            if roll < self.aggression {
                Plan::Strike
            } else if roll < self.aggression + 25 {
                Plan::Guard
            } else {
                Plan::Retreat
            }
        } else if gap >= ZONE_RANGE {
            // Far: throw something, or close the distance.
            if me.special_cooldown == 0 && roll < 55 {
                Plan::Zone
            } else if roll < 85 {
                Plan::Approach
            } else {
                Plan::Leap
            }
        } else {
            // Mid range: mostly close in.
            if roll < 70 {
                Plan::Approach
            } else if roll < 85 {
                Plan::Guard
            } else {
                Plan::Leap
            }
        }
    }
}

fn direction(sign: i32) -> Button {
    if sign >= 0 {
        Button::Right
    } else {
        Button::Left
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rollback_core::{OutputMode, Simulation};

    fn play(seed: u64, frames: usize) -> Arena {
        let mut arena = Arena::new();
        let mut p1 = ArenaBot::new(0, seed);
        let mut p2 = ArenaBot::new(1, seed);
        for _ in 0..frames {
            let inputs = [p1.decide(&arena), p2.decide(&arena)];
            arena.advance_frame(inputs, OutputMode::Present);
        }
        arena
    }

    #[test]
    fn the_same_seed_replays_the_same_match() {
        assert_eq!(play(7, 1200).checksum(), play(7, 1200).checksum());
    }

    #[test]
    fn different_seeds_diverge() {
        assert_ne!(play(7, 1200).checksum(), play(8, 1200).checksum());
    }

    #[test]
    fn the_two_sides_do_not_mirror_each_other_on_a_shared_seed() {
        let mut a = ArenaBot::new(0, 42);
        let mut b = ArenaBot::new(1, 42);
        let arena = Arena::new();
        let differed = (0..200).any(|_| a.decide(&arena) != b.decide(&arena));
        assert!(differed, "P1 and P2 bots must not act in lockstep");
    }

    #[test]
    fn bots_actually_fight() {
        let arena = play(3, 3600);
        let damage_dealt = arena.rounds_won[0] + arena.rounds_won[1] > 0
            || arena.fighters[0].health < 1000
            || arena.fighters[1].health < 1000;
        assert!(damage_dealt, "a minute of bot-vs-bot should draw blood");
    }

    #[test]
    fn a_stunned_bot_sends_neutral() {
        let mut arena = Arena::new();
        arena.fighters[1].action = Action::Hitstun;
        let mut bot = ArenaBot::new(1, 1);
        assert_eq!(bot.decide(&arena), PlayerInput::NEUTRAL);
    }

    #[test]
    fn an_incoming_projectile_makes_the_bot_guard() {
        let mut arena = Arena::new();
        // Hand-place a shot from P1 heading toward P2.
        arena.projectiles[0] = crate::arena::Projectile {
            active: true,
            x: arena.fighters[0].x,
            y: from_px(20),
            vx: from_px(6),
            owner: 0,
            ttl: 100,
        };
        let mut bot = ArenaBot::new(1, 5);
        assert!(bot.decide(&arena).contains(Button::Block));
    }

    #[test]
    fn a_bot_never_holds_both_directions() {
        let mut arena = Arena::new();
        let mut bot = ArenaBot::new(1, 11);
        for _ in 0..2000 {
            let input = bot.decide(&arena);
            assert!(
                !(input.contains(Button::Left) && input.contains(Button::Right)),
                "left+right would cancel and stall the bot"
            );
            arena.advance_frame([PlayerInput::NEUTRAL, input], OutputMode::Present);
        }
    }
}
