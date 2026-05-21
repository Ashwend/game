use std::collections::VecDeque;

use bevy::prelude::Resource;

use crate::protocol::{ToastKind, ToastMessage};

pub(crate) const MAX_VISIBLE_TOASTS: usize = 4;
pub(crate) const TOAST_VISIBLE_SECONDS: f32 = 3.4;
pub(crate) const TOAST_FADE_SECONDS: f32 = 0.45;

#[derive(Debug, Clone)]
pub(crate) struct Toast {
    pub(crate) kind: ToastKind,
    pub(crate) text: String,
    pub(crate) age: f32,
}

#[derive(Resource, Default, Debug)]
pub(crate) struct ToastState {
    toasts: VecDeque<Toast>,
}

impl ToastState {
    pub(crate) fn push_message(&mut self, message: ToastMessage) {
        self.push(message.kind, message.text);
    }

    pub(crate) fn push(&mut self, kind: ToastKind, text: impl Into<String>) {
        let text = text.into();
        if text.is_empty() {
            return;
        }

        self.toasts.push_back(Toast {
            kind,
            text,
            age: 0.0,
        });

        // Drop oldest if we exceeded the cap. Drains the front so the cap is
        // a hard ceiling on the rendered stack — fast gathers produce a
        // rolling stream rather than collapsing into one entry.
        while self.toasts.len() > MAX_VISIBLE_TOASTS {
            self.toasts.pop_front();
        }
    }

    pub(crate) fn tick(&mut self, delta_seconds: f32) {
        let total = TOAST_VISIBLE_SECONDS + TOAST_FADE_SECONDS;
        for toast in &mut self.toasts {
            toast.age += delta_seconds;
        }
        // Toasts are inserted in age order, so the oldest sit at the front
        // and expire first. `pop_front` keeps the eviction O(expired) instead
        // of `retain`'s O(n) shift.
        while self.toasts.front().is_some_and(|toast| toast.age >= total) {
            self.toasts.pop_front();
        }
    }

    pub(crate) fn clear(&mut self) {
        self.toasts.clear();
    }

    pub(crate) fn visible(&self) -> impl Iterator<Item = &Toast> {
        self.toasts.iter()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_appends_unique_toasts() {
        let mut state = ToastState::default();
        state.push(ToastKind::Success, "+3 Coal");
        state.push(ToastKind::Success, "+2 Iron Ore");

        assert_eq!(state.visible().count(), 2);
    }

    #[test]
    fn repeated_gather_events_stack_independently() {
        let mut state = ToastState::default();
        state.push(ToastKind::Success, "+4 Wood");
        state.tick(0.2);
        state.push(ToastKind::Success, "+4 Wood");
        state.push(ToastKind::Success, "+4 Wood");

        let toasts: Vec<_> = state.visible().collect();
        assert_eq!(toasts.len(), 3);
        // The newest entry should be the freshest one; oldest should have
        // accumulated tick time.
        assert!(toasts.last().unwrap().age < 0.01);
        assert!(toasts.first().unwrap().age >= 0.2);
    }

    #[test]
    fn cap_drops_oldest_toasts() {
        let mut state = ToastState::default();
        for index in 0..(MAX_VISIBLE_TOASTS + 2) {
            state.push(ToastKind::Info, format!("msg {index}"));
        }

        assert_eq!(state.visible().count(), MAX_VISIBLE_TOASTS);
        let oldest = state.visible().next().expect("front toast");
        assert_eq!(oldest.text, format!("msg {}", 2));
    }

    #[test]
    fn empty_text_is_ignored() {
        let mut state = ToastState::default();
        state.push(ToastKind::Info, "");
        assert!(state.is_empty());
    }

    #[test]
    fn tick_expires_old_toasts() {
        let mut state = ToastState::default();
        state.push(ToastKind::Info, "expiring");
        state.tick(TOAST_VISIBLE_SECONDS + TOAST_FADE_SECONDS + 0.1);
        assert!(state.is_empty());
    }
}
