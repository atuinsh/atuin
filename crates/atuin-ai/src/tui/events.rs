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
    /// Check the permission for a tool call
    CheckToolCallPermission(String),
    /// User selected a permission
    SelectPermission(PermissionResult),
    /// Continue after client tools have completed
    ContinueAfterTools,
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
    AlwaysAllowInDir,
    AlwaysAllow,
    Deny,
}
