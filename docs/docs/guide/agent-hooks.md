# AI Agent Hooks

Atuin can capture commands run by AI coding agents (like Claude Code and Codex) alongside your regular shell history. Each command is tagged with the agent that ran it, so you can filter your history by author.

## Quick Start

Install hooks for your agent, then restart the agent:

```shell
# Claude Code
atuin hook install claude-code

# Codex
atuin hook install codex
```

That's it. Commands the agent runs will now appear in your Atuin history, tagged with the agent's name.

## How It Works

AI coding agents support hook systems that notify external tools when they're about to run a shell command and when the command finishes. Atuin uses these hooks to record each command as a history entry, just like commands you type yourself.

When `atuin hook install` runs, it writes the agent's config file to register Atuin as a hook handler:

| Agent | Config file |
|-------|-------------|
| Claude Code | `~/.claude/settings.json` |
| Codex | `~/.codex/hooks.json` |

The hook lifecycle:

1. **PreToolUse** -- the agent is about to run a Bash command. Atuin records the command, working directory, and timestamp (same as `history start`).
2. **PostToolUse / PostToolUseFailure** -- the command finished. Atuin records the exit code and duration (same as `history end`).

Only `Bash` tool invocations are captured. Other tool types (file writes, web fetches, etc.) are ignored.

## Filtering by Author

By default, Atuin's interactive search shows only your own commands. Agent-run commands are hidden so they don't clutter your history.

This is controlled by the `search.authors` setting in `~/.config/atuin/config.toml`:

```toml
[search]
# Default: only show commands from human users
authors = ["$all-user"]
```

### Special filter values

| Value | Meaning |
|-------|---------|
| `$all-user` | Any author that is **not** a known AI agent |
| `$all-agent` | Any known AI agent author |

You can also use literal author names:

```toml
[search]
# Show only your own commands and Claude Code commands
authors = ["$all-user", "claude-code"]
```

```toml
[search]
# Show everything (no filtering)
authors = []
```

```toml
[search]
# Show only agent commands
authors = ["$all-agent"]
```

Currently recognized agent names are: `claude-code`, `codex`, and `copilot`.

## Supported Agents

### Claude Code

```shell
atuin hook install claude-code
```

This adds hook entries to `~/.claude/settings.json`. Claude Code calls `atuin hook claude-code` on each `Bash` tool use, passing the event as JSON on stdin.

### Codex

```shell
atuin hook install codex
```

This adds hook entries to `~/.codex/hooks.json`. Codex calls `atuin hook codex` on each Bash tool use matching `^Bash$`.

## Verifying Installation

After installing hooks and restarting your agent, run a command through the agent and then check your history:

```shell
# Show all history including agent commands
atuin search --authors '' -- ''

# Show only agent commands
atuin search --authors '$all-agent' -- ''
```

You can also check the agent's config file directly to confirm the hooks are registered:

```shell
# Claude Code
cat ~/.claude/settings.json | grep atuin

# Codex
cat ~/.codex/hooks.json | grep atuin
```

## Re-installing

Running `atuin hook install` again is safe. If hooks are already installed, the command will skip them and print a message:

```
hooks.PreToolUse: already installed, skipping
hooks.PostToolUse: already installed, skipping
hooks.PostToolUseFailure: already installed, skipping
```
