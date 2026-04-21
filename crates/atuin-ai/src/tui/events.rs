/// Application-domain events emitted by UI components.
///
/// Components translate raw key events into these semantic events,
/// which are sent via an `mpsc::Sender<AiTuiEvent>` provided through
/// eye-declare's context system. The main event loop in `inline.rs`
/// receives them and mutates `AppState` accordingly.
#[derive(Debug)]
pub(crate) enum AiTuiEvent {
    /// User updated the input text
    InputUpdated(String),
    /// User submitted text input (Enter in Input mode)
    SubmitInput(String),
    /// User entered a slash command (e.g. "/help")
    #[allow(unused)]
    SlashCommand(String),
    /// User selected a permission
    SelectPermission(PermissionResult),
    /// Cancel active generation or streaming (Esc during Generating/Streaming)
    CancelGeneration,
    /// Execute the suggested command
    ExecuteCommand,
    /// Insert command without executing
    InsertCommand,
    /// Cancel confirmation of dangerous command
    CancelConfirmation,
    /// Interrupt a running tool execution (Ctrl+C during ExecutingPreview)
    InterruptToolExecution,
    /// Retry after error
    Retry,
    /// Exit the application
    Exit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PermissionResult {
    Allow,
    /// Per-file, time-limited grant scoped to the current session.
    AllowFileForSession,
    AlwaysAllowInDir,
    AlwaysAllow,
    Deny,
}

impl PermissionResult {
    /// String identifier used as the SelectOption value.
    pub fn as_value_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::AllowFileForSession => "allow-file-session",
            Self::AlwaysAllowInDir => "always-allow-in-dir",
            Self::AlwaysAllow => "always-allow",
            Self::Deny => "deny",
        }
    }

    /// Parse from a SelectOption value string.
    pub fn from_value_str(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(Self::Allow),
            "allow-file-session" => Some(Self::AllowFileForSession),
            "always-allow-in-dir" => Some(Self::AlwaysAllowInDir),
            "always-allow" => Some(Self::AlwaysAllow),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}
