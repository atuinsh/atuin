use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::permissions::rule::Rule;

#[derive(Debug, Clone)]
pub(crate) struct RuleFile {
    pub path: PathBuf,
    pub content: RuleFileContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RuleFileContent {
    pub permissions: RuleFilePermissions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RuleFilePermissions {
    #[serde(default)]
    pub allow: Vec<Rule>,
    #[serde(default)]
    pub deny: Vec<Rule>,
    #[serde(default)]
    pub ask: Vec<Rule>,
}
