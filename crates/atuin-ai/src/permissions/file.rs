use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::permissions::rule::Rule;

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
    pub allow: Vec<Rule>,
    pub deny: Vec<Rule>,
    pub ask: Vec<Rule>,
}
