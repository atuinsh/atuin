use super::Model;

/// An action the user can take: a key (optionally with Ctrl) and the label for
/// what it does. Rendered in caret notation, e.g. `^O inspect`, `esc exit`.
#[derive(Clone, Copy)]
pub struct Action {
    pub ctrl: bool,
    pub key: &'static str,
    pub label: &'static str,
}

impl Action {
    /// An unmodified action, e.g. `esc exit`.
    pub const fn new(key: &'static str, label: &'static str) -> Self {
        Self {
            ctrl: false,
            key,
            label,
        }
    }

    /// A Ctrl-modified action, rendered with a `^` prefix, e.g. `^O inspect`.
    pub const fn ctrl(key: &'static str, label: &'static str) -> Self {
        Self {
            ctrl: true,
            key,
            label,
        }
    }

    /// Rendered width in columns: `^`(if ctrl) + key + space + label.
    pub fn width(&self) -> u16 {
        let combo = if self.ctrl { 1 + self.key.len() } else { self.key.len() };
        (combo + 1 + self.label.len()) as u16
    }
}

/// A read-only view into the [`Model`] answering "which keys can be pressed to
/// achieve what." Reduced from the model and bound to its lifetime.
#[derive(Clone, Copy)]
pub struct ActionCtx<'a> {
    model: &'a Model,
}

impl<'a> ActionCtx<'a> {
    pub(super) fn from_model(model: &'a Model) -> Self {
        Self { model }
    }

    /// The actions currently available, in display order.
    pub fn actions(&self) -> impl Iterator<Item = Action> {
        [
            Action::new("esc", "exit"),
            Action::new("tab", "edit"),
            Action::new(
                "enter",
                if self.model.enter_accept { "run" } else { "edit" },
            ),
            Action::ctrl("O", "inspect"),
        ]
        .into_iter()
    }
}
