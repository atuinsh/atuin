#[derive(Debug, Clone)]
pub(crate) struct SlashCommand {
    pub name: String,
    pub description: String,
}

impl SlashCommand {
    pub fn new(name: &str, description: &str) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
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
        registry.register(SlashCommand::new("help", "Show help information"));
        registry.register(SlashCommand::new(
            "new",
            "Start a new conversation, archiving the current one",
        ));

        registry
    }
}
