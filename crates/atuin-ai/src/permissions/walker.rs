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
        let mut to_check = self
            .start
            .ancestors()
            .map(PathBuf::from)
            .collect::<Vec<_>>();

        if let Some(global_path) = self.global_permissions_file.as_ref() {
            to_check.push(global_path.clone());
        }

        let size = to_check.len();
        let mut set: JoinSet<Result<Option<FoundRuleFile>>> = JoinSet::new();

        for (index, path) in to_check.into_iter().enumerate() {
            set.spawn(async move {
                match check_for_permissions(&path).await {
                    Ok(Some(rule_file)) => Ok(Some(FoundRuleFile {
                        depth: index,
                        file: rule_file,
                    })),
                    Ok(None) => Ok(None),
                    Err(e) => Err(e),
                }
            });
        }

        let mut found = Vec::with_capacity(size);
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
                        "Error while walking filesystem for permissions check; skipping folder: {}",
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

// Checks a directory for `.atuin/permissions.ai.toml` and returns the RuleFile if found.
// Returns None if no permissions file is found.
// Returns an error if any FS or deserialization errors occur.
async fn check_for_permissions(path: &Path) -> Result<Option<RuleFile>> {
    let permissions_file = path.join(".atuin").join("permissions.ai.toml");

    if !tokio::fs::try_exists(&permissions_file).await? {
        return Ok(None);
    }

    let content = tokio::fs::read_to_string(permissions_file).await?;
    let content: RuleFileContent = toml::from_str(&content)?;

    Ok(Some(RuleFile {
        path: path.to_path_buf(),
        content,
    }))
}
