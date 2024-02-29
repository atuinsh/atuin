use std::collections::HashMap;

use clap::ValueEnum;
use config::{builder::DefaultState, ConfigBuilder};
use eyre::Result;
use serde::Deserialize;

// Settings

#[derive(Clone, Debug, Deserialize)]
pub struct Settings {
    pub enter_accept: bool,
    pub keymap_cursor: HashMap<String, CursorStyle>,
    pub keymap_mode: KeymapMode,
    pub keymap_mode_shell: KeymapMode,
    #[serde(default)]
    pub keys: Keys,
    pub shell_up_key_binding: bool,
    pub word_jump_mode: WordJumpMode,
}

// Defaults

pub(crate) fn defaults(
    builder: ConfigBuilder<DefaultState>,
) -> Result<ConfigBuilder<DefaultState>> {
    Ok(builder
        // enter_accept defaults to false here, but true in the default config file. The dissonance is
        // intentional!
        // Existing users will get the default "False", so we don't mess with any potential
        // muscle memory.
        // New users will get the new default, that is more similar to what they are used to.
        .set_default("enter_accept", false)?
        .set_default("keymap_mode", "emacs")?
        .set_default("keymap_mode_shell", "auto")?
        .set_default("keymap_cursor", HashMap::<String, String>::new())?
        .set_default("keys.scroll_exits", true)?
        .set_default("shell_up_key_binding", false)?
        .set_default("word_jump_mode", "emacs")?)
}

#[derive(Clone, Debug, Deserialize, Copy, PartialEq, Eq, ValueEnum)]
pub enum KeymapMode {
    #[serde(rename = "emacs")]
    Emacs,

    #[serde(rename = "vim-normal")]
    VimNormal,

    #[serde(rename = "vim-insert")]
    VimInsert,

    #[serde(rename = "auto")]
    Auto,
}

impl KeymapMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            KeymapMode::Emacs => "EMACS",
            KeymapMode::VimNormal => "VIMNORMAL",
            KeymapMode::VimInsert => "VIMINSERT",
            KeymapMode::Auto => "AUTO",
        }
    }
}

// We want to translate the config to crossterm::cursor::SetCursorStyle, but
// the original type does not implement trait serde::Deserialize unfortunately.
// It seems impossible to implement Deserialize for external types when it is
// used in HashMap (https://stackoverflow.com/questions/67142663).  We instead
// define an adapter type.
#[derive(Clone, Debug, Deserialize, Copy, PartialEq, Eq, ValueEnum)]
pub enum CursorStyle {
    #[serde(rename = "default")]
    DefaultUserShape,

    #[serde(rename = "blink-block")]
    BlinkingBlock,

    #[serde(rename = "steady-block")]
    SteadyBlock,

    #[serde(rename = "blink-underline")]
    BlinkingUnderScore,

    #[serde(rename = "steady-underline")]
    SteadyUnderScore,

    #[serde(rename = "blink-bar")]
    BlinkingBar,

    #[serde(rename = "steady-bar")]
    SteadyBar,
}

impl CursorStyle {
    pub fn as_str(&self) -> &'static str {
        match self {
            CursorStyle::DefaultUserShape => "DEFAULT",
            CursorStyle::BlinkingBlock => "BLINKBLOCK",
            CursorStyle::SteadyBlock => "STEADYBLOCK",
            CursorStyle::BlinkingUnderScore => "BLINKUNDERLINE",
            CursorStyle::SteadyUnderScore => "STEADYUNDERLINE",
            CursorStyle::BlinkingBar => "BLINKBAR",
            CursorStyle::SteadyBar => "STEADYBAR",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct Keys {
    pub scroll_exits: bool,
}

#[derive(Clone, Debug, Deserialize, Copy)]
pub enum WordJumpMode {
    #[serde(rename = "emacs")]
    Emacs,

    #[serde(rename = "subl")]
    Subl,
}
