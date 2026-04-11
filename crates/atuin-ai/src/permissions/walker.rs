use std::path::{Path, PathBuf};

use eyre::Result;
use tokio::task::JoinSet;

use crate::permissions::file::{RuleFile, RuleFileContent};

#[derive(Debug)]
struct FoundRuleFile {
    depth: usize,
    file: RuleFile,
}

pub(crate) struct PermissionWalker {
    start: PathBuf,
    /// Direct path to the global permissions file (e.g. `~/.config/atuin/permissions.ai.toml`).
    global_permissions_file: Option<PathBuf>,
    rules: Vec<RuleFile>,
}

impl PermissionWalker {
    pub fn new(start: PathBuf, global_permissions_file: Option<PathBuf>) -> Self {
        Self {
            start,
            global_permissions_file,
            rules: Vec::new(),
        }
    }

    pub fn rules(&self) -> &[RuleFile] {
        &self.rules
    }

    /// Walks the filesystem starting from the start path and collecting permission files along the way.
    /// Walks to the root, then checks the global permissions file, if any.
    pub async fn walk(&mut self) -> Result<()> {
        let dirs_to_check: Vec<PathBuf> = self.start.ancestors().map(PathBuf::from).collect();
        let dir_count = dirs_to_check.len();

        let mut set: JoinSet<Result<Option<FoundRuleFile>>> = JoinSet::new();

        for (index, path) in dirs_to_check.into_iter().enumerate() {
            set.spawn(async move {
                match check_dir_for_permissions(&path).await {
                    Ok(Some(rule_file)) => Ok(Some(FoundRuleFile {
                        depth: index,
                        file: rule_file,
                    })),
                    Ok(None) => Ok(None),
                    Err(e) => Err(e),
                }
            });
        }

        // Check the global file separately (it's a direct file path, not a dir/.atuin/ pattern)
        if let Some(global_path) = self.global_permissions_file.clone() {
            let depth = dir_count; // sorts after all directory-walk entries
            set.spawn(async move {
                match load_permissions_file(&global_path).await {
                    Ok(Some(rule_file)) => Ok(Some(FoundRuleFile {
                        depth,
                        file: rule_file,
                    })),
                    Ok(None) => Ok(None),
                    Err(e) => Err(e),
                }
            });
        }

        let capacity = dir_count + usize::from(self.global_permissions_file.is_some());
        let mut found = Vec::with_capacity(capacity);
        while let Some(result) = set.join_next().await {
            let result = result?; // JoinErrors result in failure to walk the filesystem

            match result {
                Ok(Some(FoundRuleFile { depth, file })) => {
                    found.push((depth, file));
                }
                Ok(None) => {
                    continue;
                }
                Err(e) => {
                    tracing::error!(
                        "Error while walking filesystem for permissions check; skipping: {}",
                        e
                    );
                    continue;
                }
            }
        }
        // join_next() returns in order of completion, not order of spawn
        found.sort_by_key(|(depth, _)| *depth);
        self.rules = found.into_iter().map(|(_, file)| file).collect();

        Ok(())
    }
}

/// Checks a directory for `.atuin/permissions.ai.toml` and returns the RuleFile if found.
async fn check_dir_for_permissions(path: &Path) -> Result<Option<RuleFile>> {
    let file_path = path.join(".atuin").join("permissions.ai.toml");
    load_permissions_file(&file_path).await
}

/// Load a permissions file from an exact path. Returns None if the file doesn't exist.
async fn load_permissions_file(file_path: &Path) -> Result<Option<RuleFile>> {
    if !tokio::fs::try_exists(file_path).await? {
        return Ok(None);
    }

    let raw = tokio::fs::read_to_string(file_path).await?;
    let content: RuleFileContent = toml::from_str(&raw)?;

    // Use the file's parent as the rule file path (for logging/debugging)
    let path = file_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| file_path.to_path_buf());

    Ok(Some(RuleFile { path, content }))
}
