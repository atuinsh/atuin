# Atuin AI Tools & Permissions

Atuin AI has a number of tools that it can use to interact with your system, given your permission. The AI can use these tools to help answer questions and perform actions on your behalf.

!!! note "More tools coming soon"
    We will be expanding the list of tools that Atuin AI can use over time.

## Permission System

By default, Atuin AI asks your permission before using any client-side tool. You can change these defaults using a *permission file*.

### Permission Files

Permission files live at `.atuin/permissions.ai.toml` in any project. When the AI wants to run a tool, Atuin AI will check its working directory for a `.atuin/permissions.ai.toml` file, and will also check every permission file in parent directories, up to the root of the filesystem. Finally, Atuin AI checks for a `permissions.ai.toml` file in the Atuin config directory (`~/.config/atuin/permissions.ai.toml` by default).

A permission file is a TOML file with the following format:

```toml
[permissions]

allow = [
    # rules for automatically allowed tools
]

deny = [
    # rules for automatically denied tools
]

ask = [
    # rules for tools that require asking for permission
]
```

If Atuin AI doesn't find a matching rule, it defaults to asking for permission before running the tool.

Permission files found deeper in the filesystem take priority over permission files found higher up. For example, if Atuin AI finds a permission file in the current working directory that allows a tool, it will allow that tool, even if a parent directory has a permission file that denies it.

Within a permissions file, `ask` rules take priority over `deny` rules, which take priority over `allow` rules. For example, if a permission file has a rule that allows a tool, but also has a rule that asks for permission for that tool, Atuin AI will ask for permission before running the tool.

### Permission Scopes

Most rules can be scoped to a particular path or other context. For example, you can allow Atuin AI to read files in a particular directory, but not in others. For rules pertaining to file operations, the scope is a glob pattern that matches file paths.

### Example Config

Here's an example of a permission file that allows Atuin AI to read and write any markdown files in the current project, but denies it access to any `.env` files. Attempts to read or write any *other* files will result in Atuin AI requesting permission before proceeding.

!!! note "Reading and writing files"
    Atuin AI cannot currently read or write files; that capability is currently in development.

```toml
[permissions]

allow = [
    "Read(**/*.md)", "Write(**/*.md)"
]

deny = [
    "Read(.env)", "Write(.env)"
]
```

## Tools

### Atuin History

The `AtuinHistory` tool allows Atuin AI to search your Atuin history for relevant commands. This tool is read-only. Atuin AI might ask to use this tool when you ask it to recall a command or information about a command you ran in the past, or when you ask for help with a failing command (e.g. "why did my last command fail?").

![Example of Atuin History tool](../images/tool_atuin_history.png)

**Permission rule and scope:** `AtuinHistory`

**Config value:** `ai.capabilities.enable_history_search` (see [settings documentation](./settings.md#capabilities))

**Example permissions file:**

```toml
[permissions]

allow = ["AtuinHistory"]
```
