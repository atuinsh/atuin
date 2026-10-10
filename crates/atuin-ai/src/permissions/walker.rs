use std::path::{Path, PathBuf};

use atuin_common::path::PathExt as _;
use eyre::Result;
use tokio::task::JoinSet;

use crate::permissions::file::{RuleFile, RuleFileContent};

#[derive(Debug)]
struct FoundRuleFile {
    depth: usize,
    file: RuleFile,
}

pub struct PermissionWalker {
    start: PathBuf,
    /// Direct path to the global permissions file (e.g. `~/.atuin/permissions.ai.toml`).
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
            let permissions = super::writer::project_permissions_path(&path);

            if let Some(global) = &self.global_permissions_file
                && permissions.is_same_path(global)
            {
                // Avoid loading ~/.atuin/permissions.ai.toml twice.
                continue;
            }

            set.spawn(async move {
                match load_permissions_file(&permissions).await {
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
                Ok(None) => {}
                Err(e) => {
                    tracing::error!(
                        "Error while walking filesystem for permissions check; skipping: {}",
                        e
                    );
                }
            }
        }
        // join_next() returns in order of completion, not order of spawn
        found.sort_by_key(|(depth, _)| *depth);
        self.rules = found.into_iter().map(|(_, file)| file).collect();

        Ok(())
    }
}

/// Load a permissions file from an exact path. Returns None if the file doesn't exist.
async fn load_permissions_file(file_path: &Path) -> Result<Option<RuleFile>> {
    if !tokio::fs::try_exists(file_path).await? {
        return Ok(None);
    }

    let raw = tokio::fs::read_to_string(file_path).await?;
    let content: RuleFileContent = toml::from_str(&raw)?;

    // Use the file's parent as the rule file path (for logging/debugging)
    let path = file_path.parent().map(Path::to_path_buf).unwrap_or_else(|| file_path.to_path_buf());

    Ok(Some(RuleFile { path, content }))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// A `.atuin/permissions.ai.toml` above the working directory that is also the global file,
    /// as `~/.atuin/permissions.ai.toml` is by default, is loaded once. Any other is a project's,
    /// and is still loaded alongside the global one.
    #[rstest]
    #[case::global(true, 1)]
    #[case::project(false, 2)]
    #[tokio::test]
    async fn the_global_file_is_not_also_a_project_file(
        #[case] is_global: bool,
        #[case] expected: usize,
    ) {
        let home = tempfile::tempdir().unwrap();
        let dotfile = home.path().join(".atuin").join("permissions.ai.toml");
        let elsewhere = home.path().join("elsewhere").join("permissions.ai.toml");
        for file in [&dotfile, &elsewhere] {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "[permissions]\nallow = []\n").unwrap();
        }

        let global = if is_global {
            dotfile
        } else {
            elsewhere
        };
        let mut walker = PermissionWalker::new(home.path().to_path_buf(), Some(global));
        walker.walk().await.unwrap();
        assert_eq!(walker.rules().len(), expected);
    }
}
