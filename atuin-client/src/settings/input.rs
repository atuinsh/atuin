use clap::ValueEnum;
use serde::Deserialize;

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
