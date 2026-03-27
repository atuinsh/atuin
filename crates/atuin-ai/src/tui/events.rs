/// Application-domain events emitted by UI components.
///
/// Components translate raw key events into these semantic events,
/// which are sent via an `mpsc::Sender<AiTuiEvent>` provided through
/// eye-declare's context system. The main event loop in `inline.rs`
/// receives them and mutates `AppState` accordingly.
#[derive(Debug)]
pub enum AiTuiEvent {
    /// User updated the input text
    InputUpdated(String),
    /// User submitted text input (Enter in Input mode)
    SubmitInput(String),
    /// User entered a slash command (e.g. "/help")
    SlashCommand(String),
    /// Cancel active generation or streaming (Esc during Generating/Streaming)
    CancelGeneration,
    /// Execute the suggested command
    ExecuteCommand,
    /// Insert command without executing
    InsertCommand,
    /// Cancel confirmation of dangerous command
    CancelConfirmation,
    /// Retry after error
    Retry,
    /// Exit the application
    Exit,
}
