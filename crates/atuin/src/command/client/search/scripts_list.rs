use atuin_client::theme::{Meaning, Theme};
use atuin_scripts::store::script::Script;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    prelude::Alignment,
    style::{Modifier, Style},
    widgets::{
        Block, BorderType, Borders, Padding, Paragraph, StatefulWidget, Widget, block::Title,
    },
};

use super::liststate::ListState;

use crossterm::style::ContentStyle;

pub struct ScriptsList<'a> {
    scripts: &'a [Script],
    block: Option<Block<'a>>,
    theme: &'a Theme,
}

impl<'a> ScriptsList<'a> {
    pub fn new(scripts: &'a [Script], theme: &'a Theme) -> Self {
        Self {
            scripts,
            block: None,
            theme,
        }
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }
}

impl StatefulWidget for ScriptsList<'_> {
    type State = ListState;

    fn render(mut self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let list_area = self.block.take().map_or(area, |b| {
            let inner_area = b.inner(area);
            b.render(area, buf);
            inner_area
        });

        if list_area.width < 1 || list_area.height < 1 || self.scripts.is_empty() {
            return;
        }

        let list_height = list_area.height as usize;
        let selected = state.selected.min(self.scripts.len().saturating_sub(1));
        state.select(selected);

        let mut y = 0;
        for (i, script) in self.scripts.iter().enumerate() {
            if y >= list_height {
                break;
            }

            let is_selected = i == selected;
            let style = if is_selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                self.theme.as_style(Meaning::Base).into()
            };

            // Draw script name
            let script_text = if script.description.is_empty() {
                script.name.clone()
            } else {
                format!("{} - {}", script.name, script.description)
            };

            let x = list_area.left();
            let cy = list_area.top() + y as u16;
            let width = list_area.width as usize;

            buf.set_stringn(x, cy, &script_text, width, style);

            // Draw tags if available
            if !script.tags.is_empty() && y + 1 < list_height {
                let tags_text = format!("  Tags: {}", script.tags.join(", "));
                let tag_style: ContentStyle = self.theme.as_style(Meaning::Annotation).into();
                buf.set_stringn(x, cy + 1, &tags_text, width, tag_style);
                y += 2;
            } else {
                y += 1;
            }
        }

        state.max_entries = list_height;
    }
}

pub fn draw(
    f: &mut Frame,
    list_chunk: Rect,
    input_chunk: Rect,
    scripts: &Option<Vec<Script>>,
    scripts_state: &mut ListState,
    theme: &Theme,
) {
    if let Some(scripts) = scripts {
        if scripts.is_empty() {
            let message = Paragraph::new(
                "No scripts available\n\nUse 'atuin scripts new' to create your first script",
            )
            .block(
                Block::new()
                    .title(Title::from(" Scripts ".to_string()))
                    .title_alignment(Alignment::Center)
                    .borders(Borders::ALL)
                    .padding(Padding::vertical(2)),
            )
            .alignment(Alignment::Center);
            f.render_widget(message, list_chunk);
        } else {
            let scripts_list = ScriptsList::new(scripts, theme).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Scripts "),
            );
            f.render_stateful_widget(scripts_list, list_chunk, scripts_state);
        }
    } else {
        // Show loading message when scripts haven't been loaded yet
        let message = Paragraph::new("Loading scripts...")
            .block(
                Block::new()
                    .title(Title::from(" Scripts ".to_string()))
                    .title_alignment(Alignment::Center)
                    .borders(Borders::ALL)
                    .padding(Padding::vertical(2)),
            )
            .alignment(Alignment::Center);
        f.render_widget(message, list_chunk);
    }

    let feedback = Paragraph::new("Select a script and press <enter> to execute it");
    f.render_widget(feedback, input_chunk);
}
