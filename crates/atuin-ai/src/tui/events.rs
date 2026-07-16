// Used again when the permission prompt lands in the v2 port (tools slice).
#![allow(dead_code)]

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PermissionResult {
    Allow,
    /// Per-file, time-limited grant scoped to the current session.
    AllowFileForSession,
    AlwaysAllowInDir,
    AlwaysAllow,
    Deny,
}
