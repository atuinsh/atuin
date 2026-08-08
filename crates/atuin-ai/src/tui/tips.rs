//! Feature tips surfaced under the turn spinner and the "Responded in"
//! line. One tip is pulled per turn and rides through both.

use atuin_client::settings::Settings;

/// Facts a tip's relevance predicate may consult. Assembled fresh at each
/// pull so predicates see current session state, not startup state.
pub(crate) struct TipContext<'a> {
    pub settings: &'a Settings,
    /// A model is pinned — via `ai.model` or /model this session.
    pub model_set: bool,
    /// Whether context files (`TERMINAL.md`) were gathered for this
    /// invocation. `None` = not gathered yet (first request still in flight).
    pub has_context_files: Option<bool>,
}

pub(crate) struct Tip {
    /// Stable key — the hook for cross-session "already seen" persistence.
    #[cfg_attr(not(test), allow(dead_code))]
    pub id: &'static str,
    /// Rendered after a "Tip: " prefix; keep it to one short sentence.
    pub text: &'static str,
    /// `None` = always relevant.
    pub relevant: Option<fn(&TipContext) -> bool>,
}

pub(crate) const TIPS: &[Tip] = &[
    Tip {
        id: "esc-interrupt",
        text: "press Esc to interrupt a response",
        relevant: None,
    },
    Tip {
        id: "ctrl-c-interrupt",
        text: "Ctrl+C interrupts a running command",
        relevant: Some(|ctx| {
            ctx.settings
                .ai
                .capabilities
                .enable_command_execution
                .unwrap_or(true)
        }),
    },
    Tip {
        id: "model",
        text: "Atuin AI supports multiple models; try a new one with /model",
        relevant: Some(|ctx| !ctx.model_set),
    },
    Tip {
        id: "recall",
        text: "Press Up/Down to recall messages you've sent this session",
        relevant: None,
    },
    Tip {
        id: "tab-insert",
        text: "Press Tab to place a suggested command in your prompt instead of running it",
        relevant: None,
    },
    Tip {
        id: "send-cwd",
        text: "You can include your working directory in AI requests automatically: `atuin config set ai.opening.send_cwd true`",
        relevant: Some(|ctx| {
            let ai = &ctx.settings.ai;
            !ai.opening.send_cwd.or(ai.send_cwd).unwrap_or(false)
        }),
    },
    Tip {
        id: "new-session",
        text: "/new archives this session and starts fresh",
        relevant: None,
    },
    Tip {
        id: "send-last-command",
        text: "Send your last command to Atuin AI automatically with `atuin config set ai.opening.send_last_command true`",
        relevant: Some(|ctx| !ctx.settings.ai.opening.send_last_command.unwrap_or(false)),
    },
    Tip {
        id: "reload-context",
        text: "Run /reload to re-read your TERMINAL.md files mid-session",
        relevant: Some(|ctx| ctx.has_context_files == Some(true)),
    },
    Tip {
        id: "add-terminal-md",
        text: "Add a TERMINAL.md to your project to give Atuin AI persistent context",
        relevant: Some(|ctx| ctx.has_context_files == Some(false)),
    },
    Tip {
        id: "newline",
        text: "Shift+Enter or Ctrl+J inserts a newline",
        relevant: None,
    },
];

/// Walks `TIPS` in order, skipping tips whose predicate says no. The start
/// offset is randomized per session so long-running users don't always see
/// the head of the list.
pub(crate) struct TipRotation {
    cursor: usize,
}

impl TipRotation {
    pub(crate) fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as usize)
            .unwrap_or(0);
        Self::starting_at(nanos % TIPS.len().max(1))
    }

    /// Fixed start offset, for deterministic tests.
    pub(crate) fn starting_at(cursor: usize) -> Self {
        Self { cursor }
    }

    /// The next relevant tip, or `None` when tips are disabled or nothing
    /// currently applies.
    pub(crate) fn next(&mut self, ctx: &TipContext) -> Option<&'static Tip> {
        if !ctx.settings.ai.tips.unwrap_or(true) {
            return None;
        }
        for i in 0..TIPS.len() {
            let idx = (self.cursor + i) % TIPS.len();
            let tip = &TIPS[idx];
            if tip.relevant.is_none_or(|applies| applies(ctx)) {
                self.cursor = idx + 1;
                return Some(tip);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings::utc()
    }

    fn ctx(settings: &Settings) -> TipContext<'_> {
        TipContext {
            settings,
            model_set: false,
            has_context_files: None,
        }
    }

    #[test]
    fn tips_have_unique_ids() {
        let mut ids: Vec<_> = TIPS.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TIPS.len(), "duplicate tip ids");
    }

    #[test]
    fn rotation_walks_in_order_and_wraps() {
        let settings = settings();
        let c = ctx(&settings);
        // Some tips are context-gated, so the rotation only cycles through
        // the ones that apply to this context.
        let relevant_ids: Vec<&str> = TIPS
            .iter()
            .filter(|t| t.relevant.is_none_or(|applies| applies(&c)))
            .map(|t| t.id)
            .collect();
        assert!(
            relevant_ids.len() >= 2,
            "test needs at least two relevant tips"
        );

        let mut rotation = TipRotation::starting_at(0);
        // Pull two full cycles: tips come back in order, then wrap.
        for &expected in relevant_ids.iter().chain(&relevant_ids) {
            let tip = rotation.next(&c).unwrap();
            assert_eq!(tip.id, expected);
        }
    }

    #[test]
    fn irrelevant_tips_are_skipped() {
        let settings = settings();
        let model_idx = TIPS.iter().position(|t| t.id == "model").unwrap();
        let mut rotation = TipRotation::starting_at(model_idx);

        let tip = rotation
            .next(&TipContext {
                settings: &settings,
                model_set: true,
                has_context_files: None,
            })
            .unwrap();
        assert_ne!(tip.id, "model", "model tip should be skipped when set");
    }

    #[test]
    fn send_cwd_tip_respects_both_config_spellings() {
        let mut settings = settings();
        let idx = TIPS.iter().position(|t| t.id == "send-cwd").unwrap();

        settings.ai.opening.send_cwd = Some(true);
        let mut rotation = TipRotation::starting_at(idx);
        assert_ne!(rotation.next(&ctx(&settings)).unwrap().id, "send-cwd");

        settings.ai.opening.send_cwd = None;
        settings.ai.send_cwd = Some(true);
        let mut rotation = TipRotation::starting_at(idx);
        assert_ne!(rotation.next(&ctx(&settings)).unwrap().id, "send-cwd");
    }

    #[test]
    fn disabled_setting_suppresses_all_tips() {
        let mut settings = settings();
        settings.ai.tips = Some(false);
        let mut rotation = TipRotation::starting_at(0);
        assert!(rotation.next(&ctx(&settings)).is_none());
    }
}
