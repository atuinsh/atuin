//! The `[security]` section of `config.toml`: what Atuin keeps out of what it stores, on top of
//! its built-in rules.

use atuin_common::secrets::Redactor;
use atuin_common::secrets::files::SensitiveFiles;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Security {
    /// Regular expressions for credentials of your own, redacted wherever Atuin redacts: in
    /// captured command output and in AI sessions. A pattern's `secret` group is what goes, where
    /// it names one (`api-key: (?<secret>\w+)`), else all it matches.
    #[serde(with = "serde_regex", default, skip_serializing)]
    pub redact_patterns: Vec<Regex>,

    /// Files of your own whose contents are credentials, on top of the built-in ones (`.env`,
    /// `~/.ssh/id_*`, `~/.aws/credentials`, ...): what an AI agent reads out of them or writes to
    /// them is not captured, nor the output of a command naming one. A glob without a `/` is
    /// matched against a file's name (`*.secret`); one with a `/` against its path
    /// (`~/work/creds/**`).
    #[serde(default)]
    pub sensitive_files: Vec<String>,
}

impl Security {
    /// Redaction with [`Self::redact_patterns`], and the built-in patterns while `builtin`
    /// (`secrets_filter`).
    #[must_use]
    pub fn redactor(&self, builtin: bool) -> Redactor {
        Redactor::new(self.redact_patterns.clone(), builtin)
    }

    /// The files whose contents are credentials: the built-in ones, `key_path` (Atuin's own
    /// encryption key) and [`Self::sensitive_files`].
    #[must_use]
    pub fn sensitive_files(&self, key_path: &std::path::Path) -> SensitiveFiles {
        SensitiveFiles::new(&self.sensitive_files, &[key_path.to_path_buf()], dirs::home_dir())
    }
}
