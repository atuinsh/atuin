use eyre::Result;

use crate::{permissions::file::RuleFile, tools::PermissableToolCall};

pub(crate) struct PermissionRequest<'t> {
    call: &'t (dyn PermissableToolCall + Send + Sync),
}

impl<'t> PermissionRequest<'t> {
    pub fn new(call: &'t (dyn PermissableToolCall + Send + Sync)) -> Self {
        Self { call }
    }
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

    pub async fn check<'t>(
        &self,
        request: &'t PermissionRequest<'t>,
    ) -> Result<PermissionResponse> {
        // Files are in order from deepest to shallowest, so we can stop at the first match.
        // Within a file, the priority is ask -> deny -> allow
        // The first rule type that matches is the one that applies, even if a later rule would contradict it.
        for file in &self.files {
            for rule in &file.content.permissions.ask {
                if request.call.matches_rule(rule) {
                    tracing::debug!(
                        "Permission 'ASK' by rule: {} in file: {}",
                        rule,
                        file.path.display()
                    );
                    return Ok(PermissionResponse::Ask);
                }
            }

            for rule in &file.content.permissions.deny {
                if request.call.matches_rule(rule) {
                    tracing::debug!(
                        "Permission 'DENY' by rule: {} in file: {}",
                        rule,
                        file.path.display()
                    );
                    return Ok(PermissionResponse::Denied);
                }
            }

            for rule in &file.content.permissions.allow {
                if request.call.matches_rule(rule) {
                    tracing::debug!(
                        "Permission 'ALLOW' by rule: {} in file: {}",
                        rule,
                        file.path.display()
                    );
                    return Ok(PermissionResponse::Allowed);
                }
            }
        }

        Ok(PermissionResponse::Ask)
    }
}
