use std::path::PathBuf;

use eyre::Result;

use crate::{permissions::file::RuleFile, tools::PermissableToolCall};

pub(crate) struct PermissionRequest {
    working_dir: PathBuf,
    call: Box<dyn PermissableToolCall>,
}

pub(crate) enum PermissionResponse {
    Allowed,
    Denied,
    Ask,
}

pub(crate) struct PermissionChecker {
    files: Vec<RuleFile>,
}

impl PermissionChecker {
    pub fn new(files: Vec<RuleFile>) -> Self {
        Self { files }
    }

    pub async fn check(&self, request: &PermissionRequest) -> Result<PermissionResponse> {
        // Files are in order from deepest to shallowest, so we can stop at the first match.
        // Within a file, deny rules take precedence over ask and allow rules.
        // Ask rules take precedence over allow rules.
        for file in &self.files {
            for rule in &file.content.permissions.deny {
                if request.call.matches_rule(rule) {
                    return Ok(PermissionResponse::Denied);
                }
            }

            for rule in &file.content.permissions.ask {
                if request.call.matches_rule(rule) {
                    return Ok(PermissionResponse::Ask);
                }
            }

            for rule in &file.content.permissions.allow {
                if request.call.matches_rule(rule) {
                    return Ok(PermissionResponse::Allowed);
                }
            }
        }

        Ok(PermissionResponse::Ask)
    }
}
