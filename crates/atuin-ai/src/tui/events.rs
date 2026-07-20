/// A user's answer to a permission prompt, in UI vocabulary. Mapped to the
/// FSM's `PermissionChoice` at dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PermissionResult {
    Allow,
    /// Per-file, time-limited grant scoped to the current session.
    AllowFileForSession,
    AlwaysAllowInDir,
    AlwaysAllow,
    Deny,
}
