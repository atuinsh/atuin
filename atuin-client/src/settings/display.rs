use std::env;

use config::{builder::DefaultState, ConfigBuilder, Value, ValueKind};
use eyre::Result;
use ratatui::style::{Color, Style, Stylize};
use serde::{Deserialize, Deserializer};

// Settings

#[derive(Clone, Debug, Deserialize)]
pub struct Settings {
    pub inline_height: u16,
    pub invert: bool,
    pub max_preview_height: u16,
    pub prefers_reduced_motion: bool,
    pub show_preview: bool,
    pub show_help: bool,
    #[serde(alias = "display")]
    pub style: Display,
    #[serde(default)]
    pub styles: Styles,
}

// Defaults

pub(crate) fn defaults(
    builder: ConfigBuilder<DefaultState>,
) -> Result<ConfigBuilder<DefaultState>> {
    Ok(builder
        .set_default("inline_height", 0)?
        .set_default("invert", false)?
        .set_default("max_preview_height", 4)?
        .set_default(
            "prefers_reduced_motion",
            env::var("NO_MOTION")
                .ok()
                .map(|_| Value::new(None, ValueKind::Boolean(true)))
                .unwrap_or_else(|| Value::new(None, ValueKind::Boolean(false))),
        )?
        .set_default("show_preview", false)?
        .set_default("show_help", true)?
        .set_default("style", "auto")?)
}

// Display (previously Style, still "style" as a configuration value, with an
// optional alias of "display" - potentially deprecate "style" in future)

#[derive(Clone, Debug, Deserialize, Copy)]
pub enum Display {
    #[serde(rename = "auto")]
    Auto,

    #[serde(rename = "full")]
    Full,

    #[serde(rename = "compact")]
    Compact,
}

// Styles

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Styles {
    #[serde(default, deserialize_with = "Variants::deserialize_style")]
    pub command: Option<Style>,
    #[serde(default, deserialize_with = "Variants::deserialize_style")]
    pub command_selected: Option<Style>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Variants {
    Color(Color),
    Components(Components),
}

impl Variants {
    fn deserialize_style<'de, D>(deserializer: D) -> Result<Option<Style>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let variants: Option<Variants> = Deserialize::deserialize(deserializer)?;
        let style: Option<Style> = variants.map(|variants| variants.into());

        Ok(style)
    }
}

impl From<Variants> for Style {
    fn from(value: Variants) -> Style {
        match value {
            Variants::Components(complex_style) => complex_style.into(),
            Variants::Color(color) => color.into(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct Components {
    // Colors
    #[serde(default)]
    pub foreground: Option<Color>,
    #[serde(default)]
    pub background: Option<Color>,
    #[serde(default)]
    pub underline: Option<Color>,

    // Modifiers
    #[serde(default)]
    pub bold: Option<bool>,
    #[serde(default)]
    pub crossed_out: Option<bool>,
    #[serde(default)]
    pub italic: Option<bool>,
    #[serde(default)]
    pub underlined: Option<bool>,
}

impl From<Components> for Style {
    fn from(value: Components) -> Style {
        let mut style = Style::default();

        if let Some(color) = value.foreground {
            style = style.fg(color);
        };

        if let Some(color) = value.background {
            style = style.bg(color);
        }

        if let Some(color) = value.underline {
            style = style.underline_color(color);
        }

        style = match value.bold {
            Some(true) => style.bold(),
            Some(_) => style.not_bold(),
            _ => style,
        };

        style = match value.crossed_out {
            Some(true) => style.crossed_out(),
            Some(_) => style.not_crossed_out(),
            _ => style,
        };

        style = match value.italic {
            Some(true) => style.italic(),
            Some(_) => style.not_italic(),
            _ => style,
        };

        style = match value.underlined {
            Some(true) => style.underlined(),
            Some(_) => style.not_underlined(),
            _ => style,
        };

        style
    }
}
