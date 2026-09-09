use super::super::keybindings::{Action, EvalContext, Keymap};

/// Resolved bindings shared by the view selector and footer.
#[derive(Default)]
pub struct Bindings {
    keys: Vec<(Action, String)>,
    context: Option<EvalContext>,
}

impl Bindings {
    pub fn new(keymap: &Keymap, context: &EvalContext) -> Self {
        let mut bindings: Vec<_> = keymap
            .bindings
            .keys()
            .filter_map(|key| {
                keymap.resolve(key, context).map(|action| {
                    let action = match action {
                        Action::SelectPrevious => Action::InspectPrevious,
                        Action::SelectNext => Action::InspectNext,
                        action => action,
                    };
                    (action, key.to_string())
                })
            })
            .collect();

        bindings.sort_by(|(_, a), (_, b)| (a.len(), a).cmp(&(b.len(), b)));
        Self {
            keys: bindings,
            context: Some(*context),
        }
    }

    /// The keymap is fixed for the search session; only conditions can change.
    pub fn update(&mut self, keymap: &Keymap, context: &EvalContext) {
        if self.context.as_ref() != Some(context) {
            *self = Self::new(keymap, context);
        }
    }

    pub fn key(&self, action: &Action) -> Option<&str> {
        // Prefer familiar defaults when several keys do the same thing, but never
        // advertise a default that has been remapped or disabled by a condition.
        let preferred = match action {
            Action::InspectOutput => "enter",
            Action::InspectPrevious => "up",
            Action::InspectNext => "down",
            Action::InspectRuns => "r",
            Action::InspectSession => "s",
            Action::InspectStats => "t",
            Action::ReturnSelection => "tab",
            Action::Exit => "esc",
            Action::Delete => "ctrl-d",
            Action::ScrollPageUp => "pageup",
            Action::ScrollPageDown => "pagedown",
            Action::ScrollToTop => "home",
            Action::ScrollToBottom => "end",
            _ => "",
        };

        self.keys
            .iter()
            .find(|(a, key)| a == action && key == preferred)
            .or_else(|| self.keys.iter().find(|(a, _)| a == action))
            .map(|(_, key)| key.as_str())
    }

    pub fn group(&self, actions: &[Action]) -> String {
        actions
            .iter()
            .filter_map(|action| self.key(action))
            .map(|key| match key {
                "up" => "↑",
                "down" => "↓",
                "pageup" => "pgup",
                "pagedown" => "pgdn",
                key => key,
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    pub fn title(&self, name: &str, action: &Action) -> String {
        self.key(action).map_or_else(|| name.into(), |key| format!("[{key}] {name}"))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::command::client::search::keybindings::{ConditionAtom, KeyInput, KeyRule};

    #[rstest]
    fn remapped_and_conditional_keys() {
        let mut context = EvalContext {
            cursor_position: 0,
            input_width: 0,
            input_byte_len: 0,
            selected_index: 0,
            results_len: 2,
            original_input_empty: false,
            has_context: false,
        };
        let mut keymap = Keymap::new();
        keymap.bind(KeyInput::parse("enter").unwrap(), Action::Noop);
        keymap.bind(KeyInput::parse("x").unwrap(), Action::InspectOutput);
        keymap.bind(KeyInput::parse("S").unwrap(), Action::InspectSession);
        keymap.bind_conditional(KeyInput::parse("down").unwrap(), vec![KeyRule::when(
            ConditionAtom::ListAtEnd,
            Action::SelectNext,
        )]);
        let mut bindings = Bindings::default();
        bindings.update(&keymap, &context);
        assert_eq!(bindings.key(&Action::InspectOutput), Some("x"));
        assert_eq!(bindings.title("Session", &Action::InspectSession), "[S] Session");
        assert_eq!(bindings.title("Runs", &Action::InspectRuns), "Runs");
        assert!(bindings.key(&Action::InspectNext).is_none());

        let keys = bindings.keys.as_ptr();
        bindings.update(&keymap, &context);
        assert_eq!(bindings.keys.as_ptr(), keys);

        context.selected_index = 1;
        bindings.update(&keymap, &context);
        assert_eq!(bindings.key(&Action::InspectNext), Some("down"));

        context.selected_index = 0;
        bindings.update(&keymap, &context);
        assert!(bindings.key(&Action::InspectNext).is_none());
    }
}
