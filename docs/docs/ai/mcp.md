# MCP Server

Atuin ships with a built-in [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) server, giving AI agents like Claude Code, Codex, opencode, pi and Cursor access to your shell history. Your agent can look up commands you've run before, check whether they succeeded, and — with [output capture](../guide/output-capture.md) set up — read what they printed, or search that output for something it remembers seeing.

The server exposes the same history tools that [Atuin AI](./introduction.md) uses, plus output search and search over your agent [threads](./threads.md). All tools are read-only: nothing can modify or delete your history, and all data stays on your machine.

## Quick start

Connect all your configured agents with one command:

```shell
atuin mcp install
```

Atuin checks for existing user config files for Claude Code, Codex, opencode, pi and oh-my-pi (`omp`), lists the agents it found and asks once before registering the MCP server with all of them. Agents without a config file are skipped. You can also target one directly with `atuin mcp install claude-code` (or `codex`, `opencode`, `pi`, `omp`), including when its config file doesn't exist yet.

Then restart the agents you connected. They can now search your shell history.

To let it read and search what your commands printed, and your past [threads](./threads.md) with other agents, turn those on too (each also sets up the [daemon](../reference/daemon.md) they need):

```shell
atuin config enable output-capture
atuin config enable threads
```

To check it worked, ask the agent *"What's the last command I ran that failed?"*

`atuin mcp install` registers the server for your user, so it works in every project, and it uses the full path to `atuin`, so the agent doesn't need it on its `PATH`. Running it again is safe, and updates the path if `atuin` has moved.

## Setting it up by hand

The server runs over stdio: your agent starts it when it needs it, so there's nothing to keep running. Every agent just needs to run this command:

```shell
atuin mcp
```

Editors and desktop apps often don't get your shell's `PATH`. If one can't find `atuin`, replace `atuin` below with the full path that `which atuin` prints.

### Claude Code

```shell
claude mcp add --scope user atuin -- atuin mcp
```

Check it with `claude mcp list`, or `/mcp` inside Claude Code.

### Codex

```shell
codex mcp add atuin -- atuin mcp
```

Or add it to `~/.codex/config.toml` yourself:

```toml
[mcp_servers.atuin]
command = "atuin"
args = ["mcp"]
```

### opencode

Add it to `~/.config/opencode/opencode.json`:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "atuin": {
      "type": "local",
      "command": ["atuin", "mcp"],
      "enabled": true
    }
  }
}
```

Check it with `opencode mcp list`.

!!! note "opencode 2"

    Use the config above on opencode 2 as well. opencode 2.0's own `opencode mcp add` writes the server under `mcp.servers`, which opencode 2.0 doesn't load.

### pi

pi 1.0 and later:

```shell
pi mcp add atuin -- atuin mcp
```

This adds it to `~/.pi/agent/mcp.json`. Check it with `pi mcp list`.

### Cursor

Add it to `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "atuin": {
      "command": "atuin",
      "args": ["mcp"]
    }
  }
}
```

### VS Code (GitHub Copilot)

```shell
code --add-mcp '{"name":"atuin","command":"atuin","args":["mcp"]}'
```

### Gemini CLI

```shell
gemini mcp add --scope user atuin atuin mcp
```

### Claude Desktop

Open **Settings → Developer → Edit Config**, and add the server to `claude_desktop_config.json`. Claude Desktop doesn't get your shell's `PATH`, so use the full path from `which atuin`:

```json
{
  "mcpServers": {
    "atuin": {
      "command": "/full/path/to/atuin",
      "args": ["mcp"]
    }
  }
}
```

Then quit and reopen Claude Desktop.

### Zed

Add it to Zed's `settings.json`:

```json
{
  "context_servers": {
    "atuin": {
      "command": "atuin",
      "args": ["mcp"]
    }
  }
}
```

### Windsurf

Add it to `~/.codeium/windsurf/mcp_config.json`:

```json
{
  "mcpServers": {
    "atuin": {
      "command": "atuin",
      "args": ["mcp"]
    }
  }
}
```

### Anything else

Most MCP clients take a config like Cursor's above: a server named `atuin` that runs the command `atuin` with the argument `mcp`.

## Tools

### `atuin_history`

Searches your shell history. Commands containing every search term come first, and the rest of the page is filled with fuzzy matches (the same matching as the search TUI), marked as such. Each result includes the command, when and where it ran, its exit code, and its duration, along with a history ID that can be passed to `atuin_output`.

Searches can be narrowed down in a few ways:

- **Filter mode**: the same scopes as [interactive search](../guide/advanced-usage.md) — `global` (the default), `host`, `directory`, `workspace`, or `session`. The `directory` and `workspace` scopes are relative to the directory your MCP client launched the server in, which for most editors is your project directory.
- **Failed commands only**: return only commands that exited with a non-zero exit code.
- **Author**: filter to commands you ran yourself, commands run by AI agents, or commands run by one specific agent. See [AI Agent Hooks](../guide/agent-hooks.md) for how Atuin records agent-run commands.

History search reads the Atuin database directly, so it works without any extra setup.

### `atuin_output`

Fetches the captured terminal output of a previous command, identified by a history ID from `atuin_history` results. The agent can fetch specific line ranges, so it doesn't need to read a huge log to find the error at the end.

Output capture requires the [daemon](../reference/daemon.md) and [pty-proxy](../reference/pty-proxy.md) to be running — see [Capturing Command Output](../guide/output-capture.md) for setup. Without them, the tool responds with an error explaining that no output is available.

### `atuin_output_search`

Searches the captured output of every command, for when the agent knows *what* was printed but not *which* command printed it — an error message, a version string, a hostname. It's the same search as `atuin output search`, with each term in place of an argument: terms are matched as whole words and all of them must appear, and the words of one term must appear together, in that order.

Each result gives the command and its history metadata, followed by the output lines around each match with their line numbers, so the agent can pass the history ID and a line range to `atuin_output` to read more.

Like `atuin_output`, this needs output capture set up and the daemon running.

### `atuin_threads_search`, `atuin_threads_list` and `atuin_threads_read`

Search, list and read your [threads](./threads.md): the sessions of your coding agents (Claude Code, Codex, opencode and pi) that Atuin has captured. An agent can pick up where another left off, or check what was decided last time, without you explaining it again.

These need thread capture turned on with `atuin config enable threads`.

!!! note "Session scope"

    The `session` filter mode only works when the MCP server is launched from inside an Atuin-enabled shell session. Clients like editors usually launch it outside of one, in which case the other filter modes still work as normal.
