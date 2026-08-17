//! Keyboard and gamepad to [`PlayerInput`].
//!
//! Both devices are polled every frame and OR'd together, so a player can hold
//! a direction on the stick and hit a button on the keyboard without either
//! device cancelling the other.
//!
//! Reading the *held* state rather than key events is deliberate: rollback
//! needs the input as it was at a specific frame boundary, and an event queue
//! reports what happened somewhere between two frames.

use rollback_core::{Button, PlayerInput};
use sdl2::controller::{Axis, Button as PadButton, GameController};
use sdl2::keyboard::{KeyboardState, Scancode};

/// Stick deflection past which an analogue axis counts as a direction.
///
/// `i16::MAX / 2` is a deliberately large deadzone: a drifting stick that
/// registers a stray direction shows up as a misprediction on the peer, which
/// is a confusing way to discover your hardware is worn out.
const AXIS_THRESHOLD: i16 = i16::MAX / 2;

/// The keyboard layout. Documented in `docs/04-uso-local.md`.
const KEYS: &[(Scancode, Button)] = &[
    (Scancode::W, Button::Up),
    (Scancode::Up, Button::Up),
    (Scancode::S, Button::Down),
    (Scancode::Down, Button::Down),
    (Scancode::A, Button::Left),
    (Scancode::Left, Button::Left),
    (Scancode::D, Button::Right),
    (Scancode::Right, Button::Right),
    (Scancode::J, Button::Attack),
    (Scancode::K, Button::Block),
    (Scancode::L, Button::Special),
    (Scancode::U, Button::Confirm),
    (Scancode::Return, Button::Start),
    (Scancode::Space, Button::Coin),
];

const PAD_BUTTONS: &[(PadButton, Button)] = &[
    (PadButton::DPadUp, Button::Up),
    (PadButton::DPadDown, Button::Down),
    (PadButton::DPadLeft, Button::Left),
    (PadButton::DPadRight, Button::Right),
    (PadButton::X, Button::Attack),
    (PadButton::A, Button::Block),
    (PadButton::Y, Button::Special),
    (PadButton::B, Button::Confirm),
    (PadButton::Start, Button::Start),
    (PadButton::Back, Button::Coin),
];

/// Read the keyboard.
pub fn from_keyboard(state: &KeyboardState<'_>) -> PlayerInput {
    let mut input = PlayerInput::NEUTRAL;
    for &(scancode, button) in KEYS {
        if state.is_scancode_pressed(scancode) {
            input.set(button, true);
        }
    }
    input
}

/// Read a gamepad, d-pad and left stick alike.
pub fn from_controller(pad: &GameController) -> PlayerInput {
    let mut input = PlayerInput::NEUTRAL;
    for &(pad_button, button) in PAD_BUTTONS {
        if pad.button(pad_button) {
            input.set(button, true);
        }
    }
    let x = pad.axis(Axis::LeftX);
    let y = pad.axis(Axis::LeftY);
    if x <= -AXIS_THRESHOLD {
        input.set(Button::Left, true);
    }
    if x >= AXIS_THRESHOLD {
        input.set(Button::Right, true);
    }
    if y <= -AXIS_THRESHOLD {
        input.set(Button::Up, true);
    }
    if y >= AXIS_THRESHOLD {
        input.set(Button::Down, true);
    }
    input
}

/// Combine every device, then resolve impossible direction pairs.
///
/// A real arcade stick physically cannot report left and right at once. Keeping
/// that guarantee here rather than in the simulation means the simulation never
/// has to define what "both" means, and the two peers cannot disagree about it.
pub fn combine(inputs: impl IntoIterator<Item = PlayerInput>) -> PlayerInput {
    let mut merged = PlayerInput::NEUTRAL;
    for input in inputs {
        merged = PlayerInput(merged.bits() | input.bits());
    }
    resolve_socd(merged)
}

/// Simultaneous opposite cardinal directions: neutral wins.
fn resolve_socd(mut input: PlayerInput) -> PlayerInput {
    if input.contains(Button::Left) && input.contains(Button::Right) {
        input.set(Button::Left, false);
        input.set(Button::Right, false);
    }
    if input.contains(Button::Up) && input.contains(Button::Down) {
        input.set(Button::Up, false);
        input.set(Button::Down, false);
    }
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(buttons: &[Button]) -> PlayerInput {
        buttons
            .iter()
            .fold(PlayerInput::NEUTRAL, |acc, &b| acc.with(b))
    }

    #[test]
    fn combining_devices_ors_their_buttons() {
        let keyboard = make(&[Button::Left]);
        let pad = make(&[Button::Attack]);
        assert_eq!(
            combine([keyboard, pad]),
            make(&[Button::Left, Button::Attack])
        );
    }

    #[test]
    fn opposing_directions_cancel_to_neutral() {
        assert_eq!(
            combine([make(&[Button::Left]), make(&[Button::Right])]),
            PlayerInput::NEUTRAL
        );
        assert_eq!(
            combine([make(&[Button::Up, Button::Down, Button::Attack])]),
            make(&[Button::Attack]),
            "the attack survives; only the impossible pair is dropped"
        );
    }

    #[test]
    fn a_single_direction_is_untouched() {
        for b in [Button::Up, Button::Down, Button::Left, Button::Right] {
            assert_eq!(combine([make(&[b])]), make(&[b]));
        }
    }

    #[test]
    fn combining_nothing_is_neutral() {
        assert_eq!(combine([]), PlayerInput::NEUTRAL);
    }

    #[test]
    fn every_logical_button_is_reachable_from_the_keyboard() {
        let bound: std::collections::HashSet<Button> = KEYS.iter().map(|&(_, b)| b).collect();
        for b in Button::ALL {
            assert!(bound.contains(&b), "{b:?} has no key");
        }
    }

    #[test]
    fn every_logical_button_is_reachable_from_a_gamepad() {
        let bound: std::collections::HashSet<Button> =
            PAD_BUTTONS.iter().map(|&(_, b)| b).collect();
        for b in Button::ALL {
            assert!(bound.contains(&b), "{b:?} has no pad button");
        }
    }

    #[test]
    fn the_deadzone_is_large_enough_to_ignore_drift() {
        // A worn stick resting at 30% deflection must read as neutral: a stray
        // direction here shows up on the peer as a misprediction, which is a
        // confusing way to find out your hardware is failing.
        let resting_drift = (f64::from(i16::MAX) * 0.30) as i16;
        assert!(
            resting_drift < AXIS_THRESHOLD,
            "a stick resting at {resting_drift} would register a direction"
        );
    }
}
