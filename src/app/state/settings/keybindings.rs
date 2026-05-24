//! Rebindable keyboard input map. Each gameplay action carries a primary and
//! optional secondary key; persisted as stable string identifiers so the
//! `settings.json` survives a `KeyCode` reshuffle in a future Bevy release.
//!
//! Gameplay systems should always go through [`KeyBindings`] (or [`Self::pressed`]
//! / [`Self::just_pressed`] / [`Self::just_released`]) instead of touching
//! `KeyCode` directly. This keeps the rebind UI authoritative and gives us a
//! single place to query "which key is bound to X".

use std::collections::HashMap;

use bevy::{input::ButtonInput, prelude::KeyCode};
use serde::{Deserialize, Serialize};

use crate::protocol::ACTIONBAR_SLOT_COUNT;

/// One logical, rebindable action.
///
/// Variants are listed in the canonical display order used by the options
/// panel. Keep the order grouped by [`KeyBindingCategory`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum KeyAction {
    MoveForward,
    MoveBackward,
    StrafeLeft,
    StrafeRight,
    Jump,
    /// The `serde(alias = "Sprint")` keeps `settings.json` files written
    /// before the rename (when this action was called `Sprint`) loading
    /// cleanly — any custom keybinding the player saved survives.
    #[serde(alias = "Sprint")]
    Run,
    OpenChat,
    OpenInventory,
    DropItem,
    PickUp,
    PushToTalk,
    ActionbarSlot1,
    ActionbarSlot2,
    ActionbarSlot3,
    ActionbarSlot4,
    ActionbarSlot5,
    ActionbarSlot6,
    ActionbarSlot7,
    ActionbarSlot8,
    ActionbarSlot9,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum KeyBindingCategory {
    Movement,
    Combat,
    Inventory,
    Communication,
}

impl KeyBindingCategory {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Movement => "Movement",
            Self::Combat => "Combat",
            Self::Inventory => "Inventory",
            Self::Communication => "Communication",
        }
    }
}

impl KeyAction {
    /// Canonical iteration order used by the options panel.
    pub(crate) const ALL: &'static [KeyAction] = &[
        Self::MoveForward,
        Self::MoveBackward,
        Self::StrafeLeft,
        Self::StrafeRight,
        Self::Jump,
        Self::Run,
        Self::PickUp,
        Self::DropItem,
        Self::OpenInventory,
        Self::OpenChat,
        Self::PushToTalk,
        Self::ActionbarSlot1,
        Self::ActionbarSlot2,
        Self::ActionbarSlot3,
        Self::ActionbarSlot4,
        Self::ActionbarSlot5,
        Self::ActionbarSlot6,
        Self::ActionbarSlot7,
        Self::ActionbarSlot8,
        Self::ActionbarSlot9,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::MoveForward => "Move Forward",
            Self::MoveBackward => "Move Backward",
            Self::StrafeLeft => "Strafe Left",
            Self::StrafeRight => "Strafe Right",
            Self::Jump => "Jump",
            Self::Run => "Run",
            Self::OpenChat => "Open Chat",
            Self::OpenInventory => "Toggle Inventory",
            Self::DropItem => "Drop Held Item",
            Self::PickUp => "Pick Up Item",
            Self::PushToTalk => "Push To Talk",
            Self::ActionbarSlot1 => "Actionbar Slot 1",
            Self::ActionbarSlot2 => "Actionbar Slot 2",
            Self::ActionbarSlot3 => "Actionbar Slot 3",
            Self::ActionbarSlot4 => "Actionbar Slot 4",
            Self::ActionbarSlot5 => "Actionbar Slot 5",
            Self::ActionbarSlot6 => "Actionbar Slot 6",
            Self::ActionbarSlot7 => "Actionbar Slot 7",
            Self::ActionbarSlot8 => "Actionbar Slot 8",
            Self::ActionbarSlot9 => "Actionbar Slot 9",
        }
    }

    pub(crate) fn category(self) -> KeyBindingCategory {
        match self {
            Self::MoveForward | Self::MoveBackward | Self::StrafeLeft | Self::StrafeRight => {
                KeyBindingCategory::Movement
            }
            Self::Jump | Self::Run => KeyBindingCategory::Movement,
            Self::PickUp | Self::DropItem | Self::OpenInventory => KeyBindingCategory::Inventory,
            Self::OpenChat | Self::PushToTalk => KeyBindingCategory::Communication,
            Self::ActionbarSlot1
            | Self::ActionbarSlot2
            | Self::ActionbarSlot3
            | Self::ActionbarSlot4
            | Self::ActionbarSlot5
            | Self::ActionbarSlot6
            | Self::ActionbarSlot7
            | Self::ActionbarSlot8
            | Self::ActionbarSlot9 => KeyBindingCategory::Combat,
        }
    }

    pub(crate) fn default_slots(self) -> KeyBindingSlots {
        let (primary, secondary) = match self {
            Self::MoveForward => (KeyCode::KeyW, None),
            Self::MoveBackward => (KeyCode::KeyS, None),
            Self::StrafeLeft => (KeyCode::KeyA, None),
            Self::StrafeRight => (KeyCode::KeyD, None),
            Self::Jump => (KeyCode::Space, None),
            Self::Run => (KeyCode::ShiftLeft, Some(KeyCode::ShiftRight)),
            Self::OpenChat => (KeyCode::KeyT, Some(KeyCode::Enter)),
            Self::OpenInventory => (KeyCode::Tab, None),
            Self::DropItem => (KeyCode::KeyQ, None),
            Self::PickUp => (KeyCode::KeyE, None),
            Self::PushToTalk => (KeyCode::KeyV, None),
            Self::ActionbarSlot1 => (KeyCode::Digit1, None),
            Self::ActionbarSlot2 => (KeyCode::Digit2, None),
            Self::ActionbarSlot3 => (KeyCode::Digit3, None),
            Self::ActionbarSlot4 => (KeyCode::Digit4, None),
            Self::ActionbarSlot5 => (KeyCode::Digit5, None),
            Self::ActionbarSlot6 => (KeyCode::Digit6, None),
            Self::ActionbarSlot7 => (KeyCode::Digit7, None),
            Self::ActionbarSlot8 => (KeyCode::Digit8, None),
            Self::ActionbarSlot9 => (KeyCode::Digit9, None),
        };
        KeyBindingSlots {
            primary: Some(primary),
            secondary,
        }
    }
}

/// Primary and optional secondary key for an action. `None` in either slot
/// means "no key bound" — the player can leave a slot empty if they don't
/// want a secondary, or even unbind an action entirely.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KeyBindingSlots {
    #[serde(default, with = "key_code_string")]
    pub(crate) primary: Option<KeyCode>,
    #[serde(default, with = "key_code_string")]
    pub(crate) secondary: Option<KeyCode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyBindingSlot {
    Primary,
    Secondary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct KeyBindings {
    #[serde(default)]
    bindings: HashMap<KeyAction, KeyBindingSlots>,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            bindings: KeyAction::ALL
                .iter()
                .map(|action| (*action, action.default_slots()))
                .collect(),
        }
    }
}

impl KeyBindings {
    /// Backfill missing actions with their defaults so a `settings.json`
    /// written before a new action was added still loads cleanly.
    pub(crate) fn sanitized(mut self) -> Self {
        for action in KeyAction::ALL {
            self.bindings
                .entry(*action)
                .or_insert_with(|| action.default_slots());
        }
        self
    }

    pub(crate) fn slots(&self, action: KeyAction) -> KeyBindingSlots {
        self.bindings
            .get(&action)
            .copied()
            .unwrap_or_else(|| action.default_slots())
    }

    pub(crate) fn primary(&self, action: KeyAction) -> Option<KeyCode> {
        self.slots(action).primary
    }

    pub(crate) fn set(&mut self, action: KeyAction, slot: KeyBindingSlot, code: Option<KeyCode>) {
        let mut slots = self.slots(action);
        match slot {
            KeyBindingSlot::Primary => slots.primary = code,
            KeyBindingSlot::Secondary => slots.secondary = code,
        }
        self.bindings.insert(action, slots);
    }

    pub(crate) fn reset(&mut self, action: KeyAction) {
        self.bindings.insert(action, action.default_slots());
    }

    pub(crate) fn reset_all(&mut self) {
        *self = Self::default();
    }

    /// Clears `code` from every slot of every other action. Called after a
    /// successful rebind so two actions can't fire on the same key.
    pub(crate) fn clear_conflicts(&mut self, code: KeyCode, keep: KeyAction) {
        for (action, slots) in self.bindings.iter_mut() {
            if *action == keep {
                continue;
            }
            if slots.primary == Some(code) {
                slots.primary = None;
            }
            if slots.secondary == Some(code) {
                slots.secondary = None;
            }
        }
    }

    pub(crate) fn pressed(&self, action: KeyAction, keys: &ButtonInput<KeyCode>) -> bool {
        let slots = self.slots(action);
        any_key(keys, slots, ButtonInput::pressed)
    }

    pub(crate) fn just_pressed(&self, action: KeyAction, keys: &ButtonInput<KeyCode>) -> bool {
        let slots = self.slots(action);
        any_key(keys, slots, ButtonInput::just_pressed)
    }

    /// Human-readable label for a slot, e.g. "Shift Left" or "Unbound".
    pub(crate) fn slot_label(code: Option<KeyCode>) -> String {
        match code {
            Some(code) => key_code_to_string(code).unwrap_or_else(|| format!("{code:?}")),
            None => "Unbound".to_owned(),
        }
    }
}

const _: () = assert!(KeyAction::ALL.len() == 11 + ACTIONBAR_SLOT_COUNT);

fn any_key(
    keys: &ButtonInput<KeyCode>,
    slots: KeyBindingSlots,
    op: impl Fn(&ButtonInput<KeyCode>, KeyCode) -> bool,
) -> bool {
    slots.primary.is_some_and(|code| op(keys, code))
        || slots.secondary.is_some_and(|code| op(keys, code))
}

/// `Option<KeyCode>` <-> stable string identifier serde codec. Keeping the
/// on-disk form a short, human-readable string means a `KeyCode` reshuffle
/// in a future Bevy release won't silently scramble bindings.
mod key_code_string {
    use bevy::prelude::KeyCode;
    use serde::{Deserialize, Deserializer, Serializer};

    use super::{key_code_from_string, key_code_to_string};

    pub(super) fn serialize<S>(value: &Option<KeyCode>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(code) => match key_code_to_string(*code) {
                Some(s) => serializer.serialize_some(&s),
                None => serializer.serialize_none(),
            },
            None => serializer.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Option<KeyCode>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = Option::<String>::deserialize(deserializer)?;
        Ok(raw.and_then(|s| key_code_from_string(&s)))
    }
}

/// Map every `KeyCode` we expect a player to bind into a stable identifier.
/// Anything we don't recognise serializes as `None` so a saved binding
/// pointing at an unknown variant becomes "Unbound" instead of corrupting
/// the file.
pub(crate) fn key_code_to_string(code: KeyCode) -> Option<String> {
    let s = match code {
        KeyCode::KeyA => "KeyA",
        KeyCode::KeyB => "KeyB",
        KeyCode::KeyC => "KeyC",
        KeyCode::KeyD => "KeyD",
        KeyCode::KeyE => "KeyE",
        KeyCode::KeyF => "KeyF",
        KeyCode::KeyG => "KeyG",
        KeyCode::KeyH => "KeyH",
        KeyCode::KeyI => "KeyI",
        KeyCode::KeyJ => "KeyJ",
        KeyCode::KeyK => "KeyK",
        KeyCode::KeyL => "KeyL",
        KeyCode::KeyM => "KeyM",
        KeyCode::KeyN => "KeyN",
        KeyCode::KeyO => "KeyO",
        KeyCode::KeyP => "KeyP",
        KeyCode::KeyQ => "KeyQ",
        KeyCode::KeyR => "KeyR",
        KeyCode::KeyS => "KeyS",
        KeyCode::KeyT => "KeyT",
        KeyCode::KeyU => "KeyU",
        KeyCode::KeyV => "KeyV",
        KeyCode::KeyW => "KeyW",
        KeyCode::KeyX => "KeyX",
        KeyCode::KeyY => "KeyY",
        KeyCode::KeyZ => "KeyZ",
        KeyCode::Digit0 => "Digit0",
        KeyCode::Digit1 => "Digit1",
        KeyCode::Digit2 => "Digit2",
        KeyCode::Digit3 => "Digit3",
        KeyCode::Digit4 => "Digit4",
        KeyCode::Digit5 => "Digit5",
        KeyCode::Digit6 => "Digit6",
        KeyCode::Digit7 => "Digit7",
        KeyCode::Digit8 => "Digit8",
        KeyCode::Digit9 => "Digit9",
        KeyCode::Space => "Space",
        KeyCode::Tab => "Tab",
        KeyCode::Enter => "Enter",
        KeyCode::Escape => "Escape",
        KeyCode::Backspace => "Backspace",
        KeyCode::ShiftLeft => "ShiftLeft",
        KeyCode::ShiftRight => "ShiftRight",
        KeyCode::ControlLeft => "ControlLeft",
        KeyCode::ControlRight => "ControlRight",
        KeyCode::AltLeft => "AltLeft",
        KeyCode::AltRight => "AltRight",
        KeyCode::SuperLeft => "SuperLeft",
        KeyCode::SuperRight => "SuperRight",
        KeyCode::ArrowUp => "ArrowUp",
        KeyCode::ArrowDown => "ArrowDown",
        KeyCode::ArrowLeft => "ArrowLeft",
        KeyCode::ArrowRight => "ArrowRight",
        KeyCode::F1 => "F1",
        KeyCode::F2 => "F2",
        KeyCode::F3 => "F3",
        KeyCode::F4 => "F4",
        KeyCode::F5 => "F5",
        KeyCode::F6 => "F6",
        KeyCode::F7 => "F7",
        KeyCode::F8 => "F8",
        KeyCode::F9 => "F9",
        KeyCode::F10 => "F10",
        KeyCode::F11 => "F11",
        KeyCode::F12 => "F12",
        KeyCode::Backquote => "Backquote",
        KeyCode::Minus => "Minus",
        KeyCode::Equal => "Equal",
        KeyCode::BracketLeft => "BracketLeft",
        KeyCode::BracketRight => "BracketRight",
        KeyCode::Backslash => "Backslash",
        KeyCode::Semicolon => "Semicolon",
        KeyCode::Quote => "Quote",
        KeyCode::Comma => "Comma",
        KeyCode::Period => "Period",
        KeyCode::Slash => "Slash",
        KeyCode::CapsLock => "CapsLock",
        KeyCode::Insert => "Insert",
        KeyCode::Delete => "Delete",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
        KeyCode::Numpad0 => "Numpad0",
        KeyCode::Numpad1 => "Numpad1",
        KeyCode::Numpad2 => "Numpad2",
        KeyCode::Numpad3 => "Numpad3",
        KeyCode::Numpad4 => "Numpad4",
        KeyCode::Numpad5 => "Numpad5",
        KeyCode::Numpad6 => "Numpad6",
        KeyCode::Numpad7 => "Numpad7",
        KeyCode::Numpad8 => "Numpad8",
        KeyCode::Numpad9 => "Numpad9",
        KeyCode::NumpadAdd => "NumpadAdd",
        KeyCode::NumpadSubtract => "NumpadSubtract",
        KeyCode::NumpadMultiply => "NumpadMultiply",
        KeyCode::NumpadDivide => "NumpadDivide",
        KeyCode::NumpadEnter => "NumpadEnter",
        _ => return None,
    };
    Some(s.to_owned())
}

pub(crate) fn key_code_from_string(s: &str) -> Option<KeyCode> {
    Some(match s {
        "KeyA" => KeyCode::KeyA,
        "KeyB" => KeyCode::KeyB,
        "KeyC" => KeyCode::KeyC,
        "KeyD" => KeyCode::KeyD,
        "KeyE" => KeyCode::KeyE,
        "KeyF" => KeyCode::KeyF,
        "KeyG" => KeyCode::KeyG,
        "KeyH" => KeyCode::KeyH,
        "KeyI" => KeyCode::KeyI,
        "KeyJ" => KeyCode::KeyJ,
        "KeyK" => KeyCode::KeyK,
        "KeyL" => KeyCode::KeyL,
        "KeyM" => KeyCode::KeyM,
        "KeyN" => KeyCode::KeyN,
        "KeyO" => KeyCode::KeyO,
        "KeyP" => KeyCode::KeyP,
        "KeyQ" => KeyCode::KeyQ,
        "KeyR" => KeyCode::KeyR,
        "KeyS" => KeyCode::KeyS,
        "KeyT" => KeyCode::KeyT,
        "KeyU" => KeyCode::KeyU,
        "KeyV" => KeyCode::KeyV,
        "KeyW" => KeyCode::KeyW,
        "KeyX" => KeyCode::KeyX,
        "KeyY" => KeyCode::KeyY,
        "KeyZ" => KeyCode::KeyZ,
        "Digit0" => KeyCode::Digit0,
        "Digit1" => KeyCode::Digit1,
        "Digit2" => KeyCode::Digit2,
        "Digit3" => KeyCode::Digit3,
        "Digit4" => KeyCode::Digit4,
        "Digit5" => KeyCode::Digit5,
        "Digit6" => KeyCode::Digit6,
        "Digit7" => KeyCode::Digit7,
        "Digit8" => KeyCode::Digit8,
        "Digit9" => KeyCode::Digit9,
        "Space" => KeyCode::Space,
        "Tab" => KeyCode::Tab,
        "Enter" => KeyCode::Enter,
        "Escape" => KeyCode::Escape,
        "Backspace" => KeyCode::Backspace,
        "ShiftLeft" => KeyCode::ShiftLeft,
        "ShiftRight" => KeyCode::ShiftRight,
        "ControlLeft" => KeyCode::ControlLeft,
        "ControlRight" => KeyCode::ControlRight,
        "AltLeft" => KeyCode::AltLeft,
        "AltRight" => KeyCode::AltRight,
        "SuperLeft" => KeyCode::SuperLeft,
        "SuperRight" => KeyCode::SuperRight,
        "ArrowUp" => KeyCode::ArrowUp,
        "ArrowDown" => KeyCode::ArrowDown,
        "ArrowLeft" => KeyCode::ArrowLeft,
        "ArrowRight" => KeyCode::ArrowRight,
        "F1" => KeyCode::F1,
        "F2" => KeyCode::F2,
        "F3" => KeyCode::F3,
        "F4" => KeyCode::F4,
        "F5" => KeyCode::F5,
        "F6" => KeyCode::F6,
        "F7" => KeyCode::F7,
        "F8" => KeyCode::F8,
        "F9" => KeyCode::F9,
        "F10" => KeyCode::F10,
        "F11" => KeyCode::F11,
        "F12" => KeyCode::F12,
        "Backquote" => KeyCode::Backquote,
        "Minus" => KeyCode::Minus,
        "Equal" => KeyCode::Equal,
        "BracketLeft" => KeyCode::BracketLeft,
        "BracketRight" => KeyCode::BracketRight,
        "Backslash" => KeyCode::Backslash,
        "Semicolon" => KeyCode::Semicolon,
        "Quote" => KeyCode::Quote,
        "Comma" => KeyCode::Comma,
        "Period" => KeyCode::Period,
        "Slash" => KeyCode::Slash,
        "CapsLock" => KeyCode::CapsLock,
        "Insert" => KeyCode::Insert,
        "Delete" => KeyCode::Delete,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Numpad0" => KeyCode::Numpad0,
        "Numpad1" => KeyCode::Numpad1,
        "Numpad2" => KeyCode::Numpad2,
        "Numpad3" => KeyCode::Numpad3,
        "Numpad4" => KeyCode::Numpad4,
        "Numpad5" => KeyCode::Numpad5,
        "Numpad6" => KeyCode::Numpad6,
        "Numpad7" => KeyCode::Numpad7,
        "Numpad8" => KeyCode::Numpad8,
        "Numpad9" => KeyCode::Numpad9,
        "NumpadAdd" => KeyCode::NumpadAdd,
        "NumpadSubtract" => KeyCode::NumpadSubtract,
        "NumpadMultiply" => KeyCode::NumpadMultiply,
        "NumpadDivide" => KeyCode::NumpadDivide,
        "NumpadEnter" => KeyCode::NumpadEnter,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_every_action() {
        let bindings = KeyBindings::default();
        for action in KeyAction::ALL {
            assert!(bindings.primary(*action).is_some(), "{action:?}");
        }
    }

    #[test]
    fn run_has_left_and_right_shift_by_default() {
        let bindings = KeyBindings::default();
        let slots = bindings.slots(KeyAction::Run);
        assert_eq!(slots.primary, Some(KeyCode::ShiftLeft));
        assert_eq!(slots.secondary, Some(KeyCode::ShiftRight));
    }

    #[test]
    fn rebinding_clears_other_actions_using_same_key() {
        let mut bindings = KeyBindings::default();
        // Bind V (default PushToTalk) onto Jump's primary.
        bindings.set(
            KeyAction::Jump,
            KeyBindingSlot::Primary,
            Some(KeyCode::KeyV),
        );
        bindings.clear_conflicts(KeyCode::KeyV, KeyAction::Jump);

        let push_to_talk = bindings.slots(KeyAction::PushToTalk);
        assert_eq!(push_to_talk.primary, None);
        assert_eq!(bindings.primary(KeyAction::Jump), Some(KeyCode::KeyV));
    }

    #[test]
    fn pressed_checks_both_slots() {
        let mut keys = ButtonInput::default();
        keys.press(KeyCode::ShiftRight);
        let bindings = KeyBindings::default();
        assert!(bindings.pressed(KeyAction::Run, &keys));
    }

    #[test]
    fn reset_restores_default_slot() {
        let mut bindings = KeyBindings::default();
        bindings.set(
            KeyAction::Jump,
            KeyBindingSlot::Primary,
            Some(KeyCode::KeyZ),
        );
        assert_eq!(bindings.primary(KeyAction::Jump), Some(KeyCode::KeyZ));
        bindings.reset(KeyAction::Jump);
        assert_eq!(bindings.primary(KeyAction::Jump), Some(KeyCode::Space));
    }

    #[test]
    fn round_trip_through_json() {
        let mut bindings = KeyBindings::default();
        bindings.set(
            KeyAction::PushToTalk,
            KeyBindingSlot::Primary,
            Some(KeyCode::KeyB),
        );
        let json = serde_json::to_string(&bindings).unwrap();
        let parsed = serde_json::from_str::<KeyBindings>(&json)
            .unwrap()
            .sanitized();
        assert_eq!(parsed.primary(KeyAction::PushToTalk), Some(KeyCode::KeyB));
        assert_eq!(parsed.primary(KeyAction::MoveForward), Some(KeyCode::KeyW));
    }

    #[test]
    fn missing_actions_backfill_on_load() {
        // Simulate an old `settings.json` from before PushToTalk existed.
        let mut bindings = KeyBindings::default();
        bindings.bindings.remove(&KeyAction::PushToTalk);
        let restored = bindings.sanitized();
        assert_eq!(restored.primary(KeyAction::PushToTalk), Some(KeyCode::KeyV));
    }

    #[test]
    fn key_code_codec_round_trip() {
        for code in [
            KeyCode::KeyA,
            KeyCode::Digit5,
            KeyCode::Space,
            KeyCode::ShiftRight,
            KeyCode::F12,
            KeyCode::Numpad0,
        ] {
            let s = key_code_to_string(code).expect("encodable");
            assert_eq!(key_code_from_string(&s), Some(code));
        }
    }
}
