use std::path::PathBuf;

use eyre::Result;

use crate::permissions::check::{PermissionChecker, PermissionRequest, PermissionResponse};
use crate::permissions::walker::PermissionWalker;
use crate::permissions::writer;
use crate::tools::ClientToolCall;

/// Resolves permissions for client tool calls by walking the filesystem to find permission files,
pub(crate) struct PermissionResolver {
    checker: PermissionChecker,
}

impl PermissionResolver {
    /// Create a new resolver that walks from `working_dir` to root for project
    /// permissions, and also checks the global permissions file.
    pub async fn new(working_dir: PathBuf) -> Result<Self> {
        let global_file = writer::global_permissions_path();
        let mut walker = PermissionWalker::new(working_dir, Some(global_file));
        walker.walk().await?;
        let checker = PermissionChecker::new(walker.rules().to_owned());
        Ok(Self { checker })
    }

    /// Check whether `tool` is allowed, denied, or needs user confirmation.
    pub async fn check(&self, tool: &ClientToolCall) -> Result<PermissionResponse> {
        let request = PermissionRequest::new(tool);
        self.checker.check(&request).await
    }
}
