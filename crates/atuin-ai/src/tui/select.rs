//! A minimal vertical select: cursor state + keymap + view.
//!
//! The eye-declare component convention as a sub-model: the state is a
//! plain value, movement keys are a `Keymap<SelectMsg>` the parent embeds
//! with `.map()`/`.merge()`, and confirmation is deliberately absent —
//! Enter is policy, so the parent binds it and reads `cursor` itself.

use crossterm::event::KeyCode;
use eye_declare::{AnyElement, ElementExt, Keymap, col, key, keymap, text};
use ratatui_core::style::Style;

#[derive(Debug, Default)]
pub(crate) struct SelectState {
    pub cursor: usize,
}

#[derive(Debug, Clone)]
pub(crate) enum SelectMsg {
    Up,
    Down,
}

impl SelectState {
    pub fn handle(&mut self, msg: SelectMsg, len: usize) {
        if len == 0 {
            return;
        }
        self.cursor = match msg {
            SelectMsg::Up => (self.cursor + len - 1) % len,
            SelectMsg::Down => (self.cursor + 1) % len,
        };
    }

    pub fn keymap() -> Keymap<SelectMsg> {
        keymap()
            .on(key(KeyCode::Up), SelectMsg::Up)
            .on(key(KeyCode::Down), SelectMsg::Down)
    }
}

/// One row per label, the cursor row reversed.
pub(crate) fn select_view<'a>(
    labels: impl IntoIterator<Item = &'a str>,
    cursor: usize,
) -> AnyElement<'static> {
    col()
        .children(labels.into_iter().enumerate().map(|(i, label)| {
            let style = if i == cursor {
                Style::default().reversed()
            } else {
                Style::default()
            };
            text(label).style(style)
        }))
        .any()
}
