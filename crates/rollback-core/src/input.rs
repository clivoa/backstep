//! Player input representation.
//!
//! A frame of input is a single `u16` bitfield. Keeping it fixed-width and
//! `Copy` is what lets the whole session store, replay, re-send and hash inputs
//! without any allocation, and it is the unit the wire protocol repeats for
//! redundancy.

use serde::{Deserialize, Serialize};

/// One frame of input for one player.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct PlayerInput(pub u16);

impl PlayerInput {
    /// The neutral input: nothing pressed.
    pub const NEUTRAL: PlayerInput = PlayerInput(0);

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn contains(self, button: Button) -> bool {
        self.0 & button.mask() != 0
    }

    pub fn set(&mut self, button: Button, pressed: bool) {
        if pressed {
            self.0 |= button.mask();
        } else {
            self.0 &= !button.mask();
        }
    }

    pub const fn with(mut self, button: Button) -> Self {
        self.0 |= button.mask();
        self
    }
}

impl std::fmt::Debug for PlayerInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PlayerInput(0x{:04x})", self.0)
    }
}

/// Logical buttons shared by every simulation in the workspace.
///
/// The arena uses all of them; the libretro mapping in `rollback-libretro`
/// translates the same bits into the RetroPad layout, so a recorded input
/// stream means the same thing to both simulations.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[repr(u16)]
pub enum Button {
    Up = 0,
    Down = 1,
    Left = 2,
    Right = 3,
    Attack = 4,
    Block = 5,
    Special = 6,
    Start = 7,
    Coin = 8,
    Confirm = 9,
}

impl Button {
    pub const ALL: [Button; 10] = [
        Button::Up,
        Button::Down,
        Button::Left,
        Button::Right,
        Button::Attack,
        Button::Block,
        Button::Special,
        Button::Start,
        Button::Coin,
        Button::Confirm,
    ];

    pub const fn mask(self) -> u16 {
        1u16 << (self as u16)
    }
}

/// Which side of the session a player sits on. P1 is always index 0.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
pub enum PlayerHandle {
    P1,
    P2,
}

impl PlayerHandle {
    pub const fn index(self) -> usize {
        match self {
            PlayerHandle::P1 => 0,
            PlayerHandle::P2 => 1,
        }
    }

    pub const fn other(self) -> Self {
        match self {
            PlayerHandle::P1 => PlayerHandle::P2,
            PlayerHandle::P2 => PlayerHandle::P1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_have_unique_masks() {
        let mut seen = 0u16;
        for b in Button::ALL {
            assert_eq!(seen & b.mask(), 0, "{b:?} overlaps a previous mask");
            seen |= b.mask();
        }
    }

    #[test]
    fn set_and_contains_round_trip() {
        let mut input = PlayerInput::NEUTRAL;
        input.set(Button::Attack, true);
        input.set(Button::Left, true);
        assert!(input.contains(Button::Attack));
        assert!(input.contains(Button::Left));
        assert!(!input.contains(Button::Block));
        input.set(Button::Attack, false);
        assert!(!input.contains(Button::Attack));
    }

    #[test]
    fn other_handle_is_an_involution() {
        assert_eq!(PlayerHandle::P1.other(), PlayerHandle::P2);
        assert_eq!(PlayerHandle::P2.other().other(), PlayerHandle::P2);
    }
}
