use std::path::PathBuf;

use eyre::Result;

use crate::permissions::check::{PermissionChecker, PermissionRequest, PermissionResponse};
use crate::permissions::walker::PermissionWalker;
use crate::tools::ClientToolCall;

/// Resolves permissions for client tool calls by walking the filesystem to find permission files,
pub(crate) struct PermissionResolver {
    checker: PermissionChecker,
    working_dir: PathBuf,
}

impl PermissionResolver {
    pub async fn new(working_dir: PathBuf, global_dir: Option<PathBuf>) -> Result<Self> {
        let mut walker = PermissionWalker::new(working_dir.clone(), global_dir);
        walker.walk().await?;
        let checker = PermissionChecker::new(walker.rules().to_owned());
        Ok(Self {
            checker,
            working_dir,
        })
    }

    /// Check whether `tool` is allowed, denied, or needs user confirmation.
    pub async fn check(&self, tool: &ClientToolCall) -> Result<PermissionResponse> {
        let request = PermissionRequest::new(self.working_dir.clone(), Box::new(tool));
        self.checker.check(&request).await
    }
}
