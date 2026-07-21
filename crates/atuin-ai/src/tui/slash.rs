#[derive(Debug, Clone)]
pub(crate) struct SlashCommand {
    pub name: String,
    pub description: String,
    /// Built-in commands take dispatch precedence over skills with the same
    /// name; skill-backed commands are registered with `new`.
    pub is_builtin: bool,
}

impl SlashCommand {
    pub fn new(name: &str, description: &str) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            is_builtin: false,
        }
    }

    pub fn builtin(name: &str, description: &str) -> Self {
        Self {
            is_builtin: true,
            ..Self::new(name, description)
        }
    }
}

#[derive(Debug)]
pub(crate) struct SlashCommandRegistry {
    commands: Vec<SlashCommand>,
}

#[derive(Debug, Clone)]
pub(crate) struct SlashCommandSearchResult {
    pub command: SlashCommand,
    pub relevance: f32,
    pub span: (usize, usize),
}

impl SlashCommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    pub fn register(&mut self, command: SlashCommand) {
        self.commands.push(command);
    }

    pub fn get_commands(&self) -> &[SlashCommand] {
        &self.commands
    }

    pub fn contains_builtin(&self, name: &str) -> bool {
        self.commands.iter().any(|c| c.is_builtin && c.name == name)
    }

    pub fn search_fuzzy(&self, query: &str) -> Vec<SlashCommandSearchResult> {
        let query_lower = query.to_lowercase();

        self.commands
            .iter()
            .filter_map(|command| {
                let name_lower = command.name.to_lowercase();
                if let Some(start) = name_lower.find(&query_lower as &str) {
                    let end = start + query_lower.len();
                    Some((command, start, end))
                } else {
                    None
                }
            })
            .map(|(command, start, end)| {
                SlashCommandSearchResult {
                    command: command.clone(),
                    relevance: 1.0, // Simple relevance score for now
                    span: (start, end),
                }
            })
            .collect()
    }
}

impl Default for SlashCommandRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        registry.register(SlashCommand::builtin("help", "Show help information"));
        registry.register(SlashCommand::builtin(
            "model",
            "Select the AI model to use for this and future sessions",
        ));
        registry.register(SlashCommand::builtin(
            "new",
            "Start a new conversation, archiving the current one",
        ));
        registry.register(SlashCommand::builtin(
            "reload",
            "Reload context files (TERMINAL.md) on the next request",
        ));

        registry
    }
}
