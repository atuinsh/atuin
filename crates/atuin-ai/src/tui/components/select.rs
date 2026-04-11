use std::sync::mpsc;

use crossterm::event::KeyCode;
use eye_declare::{Elements, EventResult, Hooks, Span, Text, View, component, element, props};
use ratatui::style::Style;
use typed_builder::TypedBuilder;

use crate::tui::events::AiTuiEvent;

type OnSelectFn = Box<dyn Fn(&SelectOption) -> Option<AiTuiEvent> + Send + Sync + 'static>;

#[derive(TypedBuilder)]
pub(crate) struct SelectOption {
    #[builder(setter(into))]
    pub label: String,
    #[builder(setter(into))]
    pub value: String,
    #[builder(default = Style::default())]
    pub label_style: Style,
    #[builder(default = Style::default().reversed())]
    pub selected_style: Style,
}

#[derive(Default)]
pub(crate) struct PermissionSelectorState {
    selected_option: usize,
    tx: Option<mpsc::Sender<AiTuiEvent>>,
}

#[props]
pub(crate) struct Select {
    pub options: Vec<SelectOption>,
    pub on_select: OnSelectFn,
}

#[component(props = Select, state = PermissionSelectorState)]
pub(crate) fn permission_selector(
    props: &Select,
    state: &PermissionSelectorState,
    hooks: &mut Hooks<Select, PermissionSelectorState>,
) -> Elements {
    hooks.use_focusable(true);
    hooks.use_autofocus();

    hooks.use_context::<mpsc::Sender<AiTuiEvent>>(|tx, _, state| {
        state.tx = tx.cloned();
    });

    hooks.use_event(move |event, props, state| {
        if !event.is_key_press() {
            return EventResult::Ignored;
        }

        if let crossterm::event::Event::Key(key) = event {
            if key.kind != crossterm::event::KeyEventKind::Press {
                return EventResult::Ignored;
            }

            match key.code {
                KeyCode::Up => {
                    state.selected_option =
                        (state.selected_option + props.options.len() - 1) % props.options.len();
                    return EventResult::Consumed;
                }
                KeyCode::Down => {
                    state.selected_option = (state.selected_option + 1) % props.options.len();
                    return EventResult::Consumed;
                }
                KeyCode::Enter => {
                    let option = &props.options[state.selected_option];
                    if let Some(event) = (props.on_select)(option)
                        && let Some(ref tx) = state.tx
                    {
                        let _ = tx.send(event);
                    }
                    return EventResult::Consumed;
                }
                _ => {}
            }
        }

        EventResult::Ignored
    });

    element!(
        View {
            #(for (index, option) in props.options.iter().enumerate() {
                Text { Span(text: &option.label, style: if index == state.selected_option {
                    option.selected_style
                } else {
                    option.label_style
                }) }
            })
        }
    )
}
