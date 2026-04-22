# Atuin AI Tools & Permissions

Atuin AI has a number of tools that it can use to interact with your system, given your permission. The AI can use these tools to help answer questions and perform actions on your behalf.

## Permission System

By default, Atuin AI asks your permission before using any client-side tool. You can change these defaults using a _permission file_.

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

Here's an example of a permission file that allows Atuin AI to read and write any markdown files in the current project (because Write implies Read — see below), but denies it access to any `.env` files. Attempts to read or write any _other_ files will result in Atuin AI requesting permission before proceeding.

```toml
[permissions]

allow = [
    "Write(**/*.md)"
]

deny = [
    "Read(.env)"
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

### Read

The `Read` tool allows Atuin AI to read files on your system. Atuin AI might ask to use this tool when you ask it to analyze the contents of a file, when you ask for edits to the contents of a file, or when you ask a question that is most easily answered by consulting the contents of a file.

![Example of Atuin FS Tools](../images/tool_fs.png)

**Permission rule and scope:** `Read(<glob_pattern>)` (e.g. `Read(**/\*.md)`to allow reading all markdown files in the current directory and subdirectories). A missing glob pattern (e.g.`Read`) matches all files.

**Config value:** `ai.capabilities.enable_fs_tools` (see [settings documentation](./settings.md#capabilities)) — this setting enables both the `Read` and `Write` tools.

**Example permissions file:**

```toml
[permissions]
allow = ["Read(**/*.md)"]
deny = ["Read(.secret/**)"]
```

!!! warning "Write Implies Read"

    To prevent accidental data loss, Atuin AI is required to read the contents of a file before writing to it. This means that any permission rule that allows the `Write` tool for a particular file or set of files will also automatically allow the `Read` tool for those same files. For example, if you have a rule that allows `Write(**/*.md)`, Atuin AI will also be able to read any markdown files in the current directory and subdirectories, even if you don't have an explicit rule allowing `Read(**/*.md)`.

### Write

The `Write` tool allows Atuin AI to create and edit files on your system. Atuin AI might ask to use this tool when you ask it to update configuration for a tool or help debug a problem.

![Example of Atuin FS Tools](../images/tool_fs.png)

**Permission rule and scope:** `Write(<glob_pattern>)` (e.g. `Write(**/\*.md)`to allow reading all markdown files in the current directory and subdirectories). A missing glob pattern (e.g.`Write`) matches all files.

**Config value:** `ai.capabilities.enable_fs_tools` (see [settings documentation](./settings.md#capabilities)) — this setting enables both the `Read` and `Write` tools.

**Example permissions file:**

```toml
[permissions]
allow = ["Write(**/*.md)"]
deny = ["Write(.secret/**)"]
```

!!! note "File Backups"

    The first time Atuin AI writes to a file in a session, it creates a backup of the original file and stores it in Atuin's data directory, under `ai/sessions/<session_id>`. A manifest file in that directory maps the original file paths to the backup file paths. In the future, we'll be providing easier ways to recover from accidental data loss.

### Shell Command Execution

The `Shell` tool allows Atuin AI to execute shell commands on your system. Atuin AI might ask to use this tool when you ask it to perform an action that is most easily accomplished by running a shell command itself, or when you ask for help debugging a failing command, or during a multi-step workflow.

![Example of Atuin Shell Tool](../images/tool_shell.png)

**Permission rule and scope:** `Shell(<command pattern>)` (e.g. `Shell(git *)` to allow any command that starts with `git`). A missing command pattern (e.g. `Shell`) matches all commands.

**Config value:** `ai.capabilities.enable_command_execution` (see [settings documentation](./settings.md#capabilities))

**Example permissions file:**

```toml
[permissions]
allow = [
    "Shell(git add *)",
    "Shell(git commit *)"
]
```

!!! note "Command Execution Scope"

    The command pattern in a `Shell` permission rule is matched against the words in the command. The `*` wildcard has different behavior depending on where it appears:

    | Pattern | Matches | Does Not Match |
    |---------|---------|----------------|
    | `*` | Any command | — |
    | `git commit *` | `git commit`, `git commit -m "msg"` | `git`, `git push` |
    | `ls*` | `ls`, `ls -a`, `lsof` | `cat` |
    | `git * --amend` | `git commit --amend`, `git rebase --amend` | `git commit` |
    | `git commit` | `git commit` | `git`, `git push`, `git commit -m "msg"` |

    Note the difference between `ls *` (with a space) and `ls*` (without). The space-separated form uses **word-boundary** matching — `ls *` matches `ls` and `ls -a` but _not_ `lsof`. The attached form uses **prefix** matching — `ls*` matches all of those, including `lsof`.

    For `allow` and `ask` rules, a pattern without any wildcard (e.g. `git commit`) is an **exact match** — it only matches when the command words are identical. Use `git commit *` if you want to allow `git commit` with any arguments.

    For `deny` rules, a pattern without any wildcard (e.g. `rm`) is a **prefix match** — it matches any command that starts with that prefix. This means that a `deny` rule of `rm` would deny `rm`, `rm -rf /`, and `rm ./README.md` so be careful when writing `deny` rules without explicit wildcards.

!!! warning "Compound Commands"

    When the AI runs a compound command (e.g. `git add . && npm test`), Atuin parses it into individual subcommands. For a command to be automatically allowed, all subcommands must be allowed. This means that `git add . && npm test` must be enabled by both `Shell(git add *)` and `Shell(npm test)` for it to be allowed, else it would fall through and ask for permission. However, our parsing is not perfect, and there may be edge cases where it fails to correctly identify the subcommands, and some shells where command parsing is sub-par. For this reason, we recommend being cautious when allowing compound commands with broad patterns.
