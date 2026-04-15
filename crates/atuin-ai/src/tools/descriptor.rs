/// Centralized metadata for a tool type.
///
/// Covers both client-side tools (ones the CLI executes locally) and
/// server-side tools (ones the API executes remotely). This is the single
/// source of truth for display text and classification.
pub(crate) struct ToolDescriptor {
    /// Canonical wire names for this tool (the names the server sends).
    pub canonical_names: &'static [&'static str],
    /// The capability string the client must advertise for this tool to be
    /// accepted. `None` for server-side tools (always accepted).
    pub capability: Option<&'static str>,
    /// Imperative verb for permission prompts (e.g. "read", "run").
    pub display_verb: &'static str,
    /// Present-tense progressive verb for spinners (e.g. "Reading file...").
    pub progressive_verb: &'static str,
    /// Past-tense verb for summaries (e.g. "Read file").
    pub past_verb: &'static str,
    /// Whether this tool is executed client-side (by the CLI).
    pub is_client: bool,
}

// ── Client-side tool descriptors ──

pub(crate) const READ: &ToolDescriptor = &ToolDescriptor {
    canonical_names: &["read_file"],
    capability: Some("client_v1_read_file"),
    display_verb: "read",
    progressive_verb: "Reading file...",
    past_verb: "Read file",
    is_client: true,
};

pub(crate) const WRITE: &ToolDescriptor = &ToolDescriptor {
    canonical_names: &["str_replace", "file_create", "file_insert"],
    capability: Some("client_v1_write"),
    display_verb: "write to",
    progressive_verb: "Writing file...",
    past_verb: "Wrote file",
    is_client: true,
};

pub(crate) const SHELL: &ToolDescriptor = &ToolDescriptor {
    canonical_names: &["execute_shell_command"],
    capability: Some("client_v1_shell"),
    display_verb: "run",
    progressive_verb: "Running command...",
    past_verb: "Ran command",
    is_client: true,
};

pub(crate) const ATUIN_HISTORY: &ToolDescriptor = &ToolDescriptor {
    canonical_names: &["atuin_history"],
    capability: Some("client_v1_atuin_history"),
    display_verb: "search your Atuin history for",
    progressive_verb: "Searching...",
    past_verb: "Searched",
    is_client: true,
};

// ── Server-side tool descriptors ──
// These appear in tool summaries but aren't client-side tools.

pub(crate) const SERVER_SEARCH: &ToolDescriptor = &ToolDescriptor {
    canonical_names: &["web_search"],
    capability: None,
    display_verb: "search",
    progressive_verb: "Searching...",
    past_verb: "Searched",
    is_client: false,
};

pub(crate) const SERVER_SCRAPE: &ToolDescriptor = &ToolDescriptor {
    canonical_names: &["web_scrape"],
    capability: None,
    display_verb: "scrape",
    progressive_verb: "Scraping...",
    past_verb: "Scraped",
    is_client: false,
};

/// All known tool descriptors, for lookup by name.
const ALL_DESCRIPTORS: &[&ToolDescriptor] = &[
    READ,
    WRITE,
    SHELL,
    ATUIN_HISTORY,
    SERVER_SEARCH,
    SERVER_SCRAPE,
];

/// Look up a tool descriptor by its canonical wire name.
/// Returns None for unknown tool names.
pub(crate) fn by_name(name: &str) -> Option<&'static ToolDescriptor> {
    ALL_DESCRIPTORS
        .iter()
        .find(|d| d.canonical_names.contains(&name))
        .copied()
}
