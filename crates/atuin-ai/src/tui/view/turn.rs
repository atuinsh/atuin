use crate::tui::ConversationEvent;

#[derive(Debug)]
pub(crate) enum DangerLevel {
    Low(Option<String>),
    Medium(Option<String>),
    High(Option<String>),
    Unknown(Option<String>),
}

impl DangerLevel {
    pub(crate) fn notes(&self) -> Option<&String> {
        match self {
            DangerLevel::Low(notes) => notes.as_ref(),
            DangerLevel::Medium(notes) => notes.as_ref(),
            DangerLevel::High(notes) => notes.as_ref(),
            DangerLevel::Unknown(notes) => notes.as_ref(),
        }
    }
}

impl From<(&String, &String)> for DangerLevel {
    fn from((danger_level, danger_notes): (&String, &String)) -> Self {
        let notes = if danger_notes.is_empty() {
            None
        } else {
            Some(danger_notes.to_string())
        };

        match danger_level.as_str() {
            "low" => DangerLevel::Low(notes),
            "medium" => DangerLevel::Medium(notes),
            "med" => DangerLevel::Medium(notes),
            "high" => DangerLevel::High(notes),
            _ => DangerLevel::Unknown(notes),
        }
    }
}

#[derive(Debug)]
pub(crate) enum ConfidenceLevel {
    Low(Option<String>),
    Medium(Option<String>),
    High(Option<String>),
    Unknown(Option<String>),
}

impl ConfidenceLevel {
    pub(crate) fn notes(&self) -> Option<&String> {
        match self {
            ConfidenceLevel::Low(notes) => notes.as_ref(),
            ConfidenceLevel::Medium(notes) => notes.as_ref(),
            ConfidenceLevel::High(notes) => notes.as_ref(),
            ConfidenceLevel::Unknown(notes) => notes.as_ref(),
        }
    }
}

impl From<(&String, &String)> for ConfidenceLevel {
    fn from((confidence_level, confidence_notes): (&String, &String)) -> Self {
        let notes = if confidence_notes.is_empty() {
            None
        } else {
            Some(confidence_notes.to_string())
        };

        match confidence_level.as_str() {
            "low" => ConfidenceLevel::Low(notes),
            "medium" => ConfidenceLevel::Medium(notes),
            "med" => ConfidenceLevel::Medium(notes),
            "high" => ConfidenceLevel::High(notes),
            _ => ConfidenceLevel::Unknown(notes),
        }
    }
}

#[derive(Debug)]
pub(crate) enum UiEvent {
    Text { content: String },
    ToolCall(ToolCallDetails),
    ToolSummary(ToolSummary),
    SuggestedCommand(SuggestedCommandDetails),
    OutOfBandOutput(OutOfBandOutputDetails),
}

#[derive(Debug)]
pub(crate) struct ToolCallDetails {
    tool_use_id: String,
    name: String,
    status: ToolResultStatus,
}

#[derive(Debug)]
pub(crate) struct SuggestedCommandDetails {
    pub(crate) command: String,
    pub(crate) danger_level: DangerLevel,
    pub(crate) confidence_level: ConfidenceLevel,
    pub(crate) first_event_in_turn: bool,
}

#[derive(Debug)]
pub(crate) struct OutOfBandOutputDetails {
    pub(crate) command: Option<String>,
    pub(crate) content: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ToolResultStatus {
    Pending,
    Success,
    Error,
}

#[derive(Debug)]
pub(crate) enum UiTurn {
    User { events: Vec<UiEvent> },
    Agent { events: Vec<UiEvent> },
    OutOfBand { events: Vec<UiEvent> },
}

pub(crate) struct TurnBuilder {
    turns: Vec<UiTurn>,
    current_turn: Option<UiTurn>,
}

impl TurnBuilder {
    pub(crate) fn new() -> Self {
        Self {
            turns: Vec::new(),
            current_turn: None,
        }
    }

    pub(crate) fn add_event(&mut self, event: &ConversationEvent) {
        match event {
            ConversationEvent::UserMessage { content } => {
                self.add_user_message(content);
            }
            ConversationEvent::Text { content } => {
                self.add_agent_text(content);
            }
            ConversationEvent::ToolCall { id, name, input } => {
                if name == "suggest_command" {
                    self.add_suggested_command(input);
                } else {
                    self.add_tool_call(id, name, input);
                }
            }
            ConversationEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                self.add_tool_result(tool_use_id, content, *is_error);
            }
            ConversationEvent::OutOfBandOutput {
                name,
                command,
                content,
            } => {
                self.add_out_of_band_output(name, command.as_deref(), content);
            }
        }
    }

    pub(crate) fn build(&mut self) -> Vec<UiTurn> {
        self.commit_turn();

        // Collapse consecutive tool calls within each agent turn into ToolSummary
        for turn in &mut self.turns {
            if let UiTurn::Agent { events } = turn {
                let mut new_events: Vec<UiEvent> = Vec::new();
                let mut pending_tools: Vec<ToolCallDetails> = Vec::new();

                for event in events.drain(..) {
                    match event {
                        UiEvent::ToolCall(details) => {
                            pending_tools.push(details);
                        }
                        other => {
                            if !pending_tools.is_empty() {
                                new_events.push(UiEvent::ToolSummary(ToolSummary {
                                    tool_calls: std::mem::take(&mut pending_tools),
                                }));
                            }
                            new_events.push(other);
                        }
                    }
                }

                if !pending_tools.is_empty() {
                    new_events.push(UiEvent::ToolSummary(ToolSummary {
                        tool_calls: pending_tools,
                    }));
                }

                *events = new_events;
            }
        }

        std::mem::take(&mut self.turns)
    }

    fn commit_turn(&mut self) {
        if let Some(turn) = self.current_turn.take() {
            self.turns.push(turn);
        }
    }

    fn start_user_turn(&mut self) {
        if !matches!(self.current_turn, Some(UiTurn::User { .. })) {
            self.commit_turn();
            self.current_turn = Some(UiTurn::User { events: vec![] });
        }
    }

    fn start_agent_turn(&mut self) {
        if !matches!(self.current_turn, Some(UiTurn::Agent { .. })) {
            self.commit_turn();
            self.current_turn = Some(UiTurn::Agent { events: vec![] });
        }
    }

    fn start_out_of_band_turn(&mut self) {
        if !matches!(self.current_turn, Some(UiTurn::OutOfBand { .. })) {
            self.commit_turn();
            self.current_turn = Some(UiTurn::OutOfBand { events: vec![] });
        }
    }

    fn turn_mut_unsafe(&mut self) -> &mut UiTurn {
        self.current_turn.as_mut().unwrap()
    }

    fn add_user_message(&mut self, content: &str) {
        self.start_user_turn();
        if let UiTurn::User { events } = self.turn_mut_unsafe() {
            events.push(UiEvent::Text {
                content: content.to_string(),
            });
        }
    }

    fn add_agent_text(&mut self, content: &str) {
        self.start_agent_turn();
        if let UiTurn::Agent { events } = self.turn_mut_unsafe() {
            events.push(UiEvent::Text {
                content: content.to_string(),
            });
        }
    }

    fn add_suggested_command(&mut self, input: &serde_json::Value) {
        let command = input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if command.is_empty() {
            return;
        }

        self.start_agent_turn();
        if let UiTurn::Agent { events } = self.turn_mut_unsafe() {
            let danger_level = input
                .get("danger")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let confidence_level = input
                .get("confidence")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let danger_notes = input
                .get("danger_notes")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let confidence_notes = input
                .get("confidence_notes")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let danger = DangerLevel::from((&danger_level, &danger_notes));
            let confidence = ConfidenceLevel::from((&confidence_level, &confidence_notes));

            let first_event_in_turn = events.is_empty();

            events.push(UiEvent::SuggestedCommand(SuggestedCommandDetails {
                command: input
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                danger_level: danger,
                confidence_level: confidence,
                first_event_in_turn,
            }));
        }
    }

    fn add_tool_call(&mut self, id: &str, name: &str, _input: &serde_json::Value) {
        self.start_agent_turn();
        if let UiTurn::Agent { events } = self.turn_mut_unsafe() {
            events.push(UiEvent::ToolCall(ToolCallDetails {
                tool_use_id: id.to_string(),
                name: name.to_string(),
                status: ToolResultStatus::Pending,
            }));
        }
    }

    fn add_tool_result(&mut self, tool_use_id: &str, _content: &str, is_error: bool) {
        self.start_agent_turn();
        if let UiTurn::Agent { events } = self.turn_mut_unsafe() {
            let event = events.iter_mut().find(|e| match e {
                UiEvent::ToolCall(ToolCallDetails {
                    tool_use_id: id, ..
                }) => id == tool_use_id,
                _ => false,
            });
            if let Some(UiEvent::ToolCall(ToolCallDetails { status, .. })) = event {
                *status = if is_error {
                    ToolResultStatus::Error
                } else {
                    ToolResultStatus::Success
                };
            }
        }
    }

    fn add_out_of_band_output(&mut self, _name: &str, command: Option<&str>, content: &str) {
        self.start_out_of_band_turn();
        if let UiTurn::OutOfBand { events } = self.turn_mut_unsafe() {
            events.push(UiEvent::OutOfBandOutput(OutOfBandOutputDetails {
                command: command.map(|c| c.to_string()),
                content: content.to_string(),
            }));
        }
    }
}

#[derive(Debug)]
pub(crate) struct ToolSummary {
    tool_calls: Vec<ToolCallDetails>,
}

impl ToolSummary {
    /// Determines the summary line:
    /// - If any call is pending, use present tense verb with `-ing`
    /// - If multiple calls are complete, say "Used n tools"
    /// - If a single call is complete, use past tense verb
    pub(crate) fn summary(&self) -> String {
        if self.any_pending() {
            // Find the last pending tool for the active verb
            if let Some(pending) = self
                .tool_calls
                .iter()
                .rev()
                .find(|t| t.status == ToolResultStatus::Pending)
            {
                return Self::progressive_verb(&pending.name);
            }
        }

        if self.tool_calls.len() == 1 {
            return Self::past_verb(&self.tool_calls[0].name);
        }

        format!("Used {} tools", self.tool_calls.len())
    }

    /// Determines if the spinner should be spinning
    pub(crate) fn any_pending(&self) -> bool {
        self.tool_calls
            .iter()
            .any(|tool_call| tool_call.status == ToolResultStatus::Pending)
    }

    /// Present-tense progressive verb for a tool name (e.g. "Searching...")
    fn progressive_verb(name: &str) -> String {
        match name {
            "search" => "Searching...".into(),
            "read" | "read_file" => "Reading file...".into(),
            "write" | "write_file" => "Writing file...".into(),
            "execute" | "run" | "bash" => "Running command...".into(),
            "list" | "list_files" => "Listing files...".into(),
            _ => format!("Running {}...", name.replace('_', " ")),
        }
    }

    /// Past-tense verb for a tool name (e.g. "Searched")
    fn past_verb(name: &str) -> String {
        match name {
            "search" => "Searched".into(),
            "read" | "read_file" => "Read file".into(),
            "write" | "write_file" => "Wrote file".into(),
            "execute" | "run" | "bash" => "Ran command".into(),
            "list" | "list_files" => "Listed files".into(),
            _ => format!("Ran {}", name.replace('_', " ")),
        }
    }
}
