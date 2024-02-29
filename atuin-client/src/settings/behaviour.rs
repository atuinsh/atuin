use clap::ValueEnum;
use config::{builder::DefaultState, ConfigBuilder};
use eyre::Result;
use serde::Deserialize;

// Settings

#[derive(Clone, Debug, Deserialize)]
pub struct Settings {
    pub exit_mode: ExitMode,
    pub filter_mode: FilterMode,
    pub filter_mode_shell_up_key_binding: Option<FilterMode>,
    pub search_mode: SearchMode,
    pub search_mode_shell_up_key_binding: Option<SearchMode>,
}

// Defaults

pub(crate) fn defaults(
    builder: ConfigBuilder<DefaultState>,
) -> Result<ConfigBuilder<DefaultState>> {
    Ok(builder
        .set_default("search_mode", "fuzzy")?
        .set_default("filter_mode", "global")?
        .set_default("exit_mode", "return-original")?)
}

// Exit

#[derive(Clone, Debug, Deserialize, Copy)]
pub enum ExitMode {
    #[serde(rename = "return-original")]
    ReturnOriginal,

    #[serde(rename = "return-query")]
    ReturnQuery,
}

// Filter

#[derive(Clone, Debug, Deserialize, Copy, PartialEq, Eq, ValueEnum)]
pub enum FilterMode {
    #[serde(rename = "global")]
    Global = 0,

    #[serde(rename = "host")]
    Host = 1,

    #[serde(rename = "session")]
    Session = 2,

    #[serde(rename = "directory")]
    Directory = 3,

    #[serde(rename = "workspace")]
    Workspace = 4,
}

impl FilterMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            FilterMode::Global => "GLOBAL",
            FilterMode::Host => "HOST",
            FilterMode::Session => "SESSION",
            FilterMode::Directory => "DIRECTORY",
            FilterMode::Workspace => "WORKSPACE",
        }
    }
}

// Search

#[derive(Clone, Debug, Deserialize, Copy, ValueEnum, PartialEq)]
pub enum SearchMode {
    #[serde(rename = "prefix")]
    Prefix,

    #[serde(rename = "fulltext")]
    #[clap(aliases = &["fulltext"])]
    FullText,

    #[serde(rename = "fuzzy")]
    Fuzzy,

    #[serde(rename = "skim")]
    Skim,
}

impl SearchMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SearchMode::Prefix => "PREFIX",
            SearchMode::FullText => "FULLTXT",
            SearchMode::Fuzzy => "FUZZY",
            SearchMode::Skim => "SKIM",
        }
    }
    pub fn next(&self, super::Settings { behaviour, .. }: &super::Settings) -> Self {
        match self {
            SearchMode::Prefix => SearchMode::FullText,
            // if the user is using skim, we go to skim
            SearchMode::FullText if behaviour.search_mode == SearchMode::Skim => SearchMode::Skim,
            // otherwise fuzzy.
            SearchMode::FullText => SearchMode::Fuzzy,
            SearchMode::Fuzzy | SearchMode::Skim => SearchMode::Prefix,
        }
    }
}
