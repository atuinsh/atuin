use crate::permissions::file::RuleFile;
use crate::tools::PermissibleToolCall;

pub struct PermissionRequest<'t> {
    call: &'t (dyn PermissibleToolCall + Send + Sync),
}

impl<'t> PermissionRequest<'t> {
    pub fn new(call: &'t (dyn PermissibleToolCall + Send + Sync)) -> Self {
        Self { call }
    }
}

pub enum PermissionResponse {
    Allowed,
    Denied,
    Ask,
}

pub struct PermissionChecker {
    files: Vec<RuleFile>,
}

impl PermissionChecker {
    pub fn new(files: Vec<RuleFile>) -> Self {
        Self { files }
    }

    #[must_use]
    pub fn check<'t>(&self, request: &'t PermissionRequest<'t>) -> PermissionResponse {
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
                    return PermissionResponse::Ask;
                }
            }

            for rule in &file.content.permissions.deny {
                if request.call.matches_rule(rule) {
                    tracing::debug!(
                        "Permission 'DENY' by rule: {} in file: {}",
                        rule,
                        file.path.display()
                    );
                    return PermissionResponse::Denied;
                }
            }

            if request.call.all_covered_by(&file.content.permissions.allow) {
                tracing::debug!("Permission 'ALLOW' by rules in file: {}", file.path.display());
                return PermissionResponse::Allowed;
            }
        }

        PermissionResponse::Ask
    }
}
