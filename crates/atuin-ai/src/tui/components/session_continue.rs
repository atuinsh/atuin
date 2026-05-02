use chrono_humanize::HumanTime;
use eye_declare::{Elements, Hooks, Span, Text, component, element, props};
use ratatui::style::{Color, Modifier, Style};

#[props]
pub(crate) struct SessionContinue {
    pub continued_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Default)]
pub(crate) struct SessionContinueState {
    /// Frozen on mount so the label doesn't change on every render.
    label: Option<String>,
}

#[component(props = SessionContinue, state = SessionContinueState)]
fn session_continue(
    _props: &SessionContinue,
    state: &SessionContinueState,
    hooks: &mut Hooks<SessionContinue, SessionContinueState>,
) -> Elements {
    hooks.use_mount(|props, state| {
        state.label = Some(match props.continued_at {
            Some(t) => {
                let human = HumanTime::from(t - chrono::Utc::now());
                format!(
                    "  Continuing previous session (last active {human}) - type /new to start a new session"
                )
            }
            None => {
                "  Continuing previous session - type /new to start a new session".to_string()
            }
        });
    });

    let resume_label = state
        .label
        .as_deref()
        .unwrap_or("  Continuing previous session - type /new to start a new session");

    element! {
        Text {
            Span(
                text: resume_label,
                style: Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
            )
        }
    }
}
