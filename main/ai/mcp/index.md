# MCP Server

Atuin ships with a built-in [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) server, giving AI agents like Claude Code, Codex, Hermes Agent, Kiro CLI, opencode, pi and Cursor access to your shell history. Your agent can look up commands you've run before, check whether they succeeded, and — with [output capture](https://docs.atuin.sh/guide/output-capture/index.md) set up — read what they printed, or search that output for something it remembers seeing.

The server exposes history search, command output and output search, and search over your agent [threads](https://docs.atuin.sh/ai/threads/index.md). All tools are read-only: nothing can modify or delete your history, and all data stays on your machine.

## Quick start

Connect your agents with one command:

```
atuin setup --only --mcp
```

This registers the MCP server with every coding agent that setup finds on this machine, such as Amp, Claude Code, Codex, GitHub Copilot CLI (`copilot`), Grok Build (`grok`), Hermes Agent (`hermes`), Kiro CLI, opencode, pi and oh-my-pi (`omp`). An agent counts when its program is on your `PATH` or it has sessions here. To connect only some, name them with `--agents`, as `atuin setup --only --mcp --agents claude-code`.

Then restart the agents you connected. They can now search your shell history.

To let them read and search what your commands printed, and your past [threads](https://docs.atuin.sh/ai/threads/index.md) with other agents, turn those on too, along with the [daemon](https://docs.atuin.sh/reference/daemon/index.md) they need:

```
atuin setup --only --output-capture --threads --daemon
```

To check it worked, ask the agent *"What's the last command I ran that failed?"*

Setup registers the server for your user, so it works in every project, and it uses the full path to `atuin`, so the agent doesn't need it on its `PATH`. Running it again is safe, and updates the path if `atuin` has moved.

To disconnect your agents, run `atuin setup --only --no-mcp`, or clear them under Connected agents on the setup screen. Setup takes the server out of Hermes with `hermes config unset mcp_servers.atuin`. If `hermes` isn't on your `PATH`, or is too old to have `config unset`, setup tells you to remove `mcp_servers.atuin` from Hermes's `config.yaml` yourself.

### Hermes Agent

Setup adds the server to the `config.yaml` of the Hermes home that `hermes` uses. By default, that's `~/.hermes/config.yaml`, or the active profile's. A running Hermes gateway picks the server up within about a minute, with no restart. Check it with `hermes mcp list`. Hermes names the tools `mcp__atuin__atuin_history` and so on.

Each profile has its own `config.yaml`. To add the server to another profile, run setup with `HERMES_HOME` set to the profile's directory:

```
HERMES_HOME=~/.hermes/profiles/work atuin setup --only --mcp --agents hermes
```

Hermes gives an MCP server only a few of its environment variables (`PATH`, `HOME`, `XDG_*` and some others), never `ATUIN_*` ones. To make up for that, setup copies the `ATUIN_*` settings in your shell that say where Atuin's config and data are into the server's `env`: those ending in `_HOME`, `_DIR` or `_PATH`, such as `ATUIN_HOME`, `ATUIN_CONFIG_DIR` or `ATUIN_DB_PATH`. It copies nothing else, so no secret reaches Hermes's config. Run `atuin setup --only --mcp` again after you change one. If your config's paths use other variables, such as `$HISTDIR`, add them to its `env` yourself, or it reads the default Atuin data:

```
hermes config set mcp_servers.atuin.env '{"ATUIN_CONFIG_DIR": "/path/to/config"}'
```

## Setting it up by hand

The server runs over stdio: your agent starts it when it needs it, so there's nothing to keep running. Every agent just needs to run this command:

```
atuin mcp
```

Setup registers this command with the coding agents listed in [Quick start](#quick-start). To connect one of them, name it with `--agents`. For example, `atuin setup --only --mcp --agents codex` connects Codex. Add the server to the editors and desktop apps below yourself.

Editors and desktop apps often don't get your shell's `PATH`. If one can't find `atuin`, replace `atuin` below with the full path that `which atuin` prints.

### Cursor

Add it to `~/.cursor/mcp.json`:

```
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

```
code --add-mcp '{"name":"atuin","command":"atuin","args":["mcp"]}'
```

### Gemini CLI

```
gemini mcp add --scope user atuin atuin mcp
```

### Claude Desktop

Open **Settings → Developer → Edit Config**, and add the server to `claude_desktop_config.json`. Claude Desktop doesn't get your shell's `PATH`, so use the full path from `which atuin`:

```
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

```
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

```
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

- **Filter mode**: the same scopes as [interactive search](https://docs.atuin.sh/guide/advanced-usage/index.md) — `global` (the default), `host`, `directory`, `workspace`, or `session`. The `directory` and `workspace` scopes are relative to the directory your MCP client launched the server in, which for most editors is your project directory.
- **Failed commands only**: return only commands that exited with a non-zero exit code.
- **Author**: filter to commands you ran yourself, commands run by AI agents, or commands run by one specific agent. See [AI Agent Hooks](https://docs.atuin.sh/guide/agent-hooks/index.md) for how Atuin records agent-run commands.

History search reads the Atuin database directly, so it works without any extra setup.

### `atuin_output`

Fetches the captured terminal output of a previous command, identified by a history ID from `atuin_history` results. The agent can fetch specific line ranges, so it doesn't need to read a huge log to find the error at the end.

Output capture requires the [daemon](https://docs.atuin.sh/reference/daemon/index.md) and [pty-proxy](https://docs.atuin.sh/reference/pty-proxy/index.md) to be running — see [Capturing Command Output](https://docs.atuin.sh/guide/output-capture/index.md) for setup. Without them, the tool responds with an error explaining that no output is available.

### `atuin_output_search`

Searches the captured output of every command, for when the agent knows *what* was printed but not *which* command printed it — an error message, a version string, a hostname. It's the same search as `atuin output search`, with each term in place of an argument: terms are matched as whole words and all of them must appear, and the words of one term must appear together, in that order.

Each result gives the command and its history metadata, followed by the output lines around each match with their line numbers, so the agent can pass the history ID and a line range to `atuin_output` to read more.

Like `atuin_output`, this needs output capture set up and the daemon running.

### `atuin_threads_search`, `atuin_threads_list` and `atuin_threads_read`

Search, list and read your [threads](https://docs.atuin.sh/ai/threads/index.md): the sessions of your coding agents (Claude Code, Codex, Copilot, Hermes Agent, Kiro CLI, opencode and pi) that Atuin has captured. An agent can pick up where another left off, or check what was decided last time, without you explaining it again.

These need thread capture, which `atuin setup --only --threads --daemon` turns on.

Session scope

The `session` filter mode only works when the MCP server is launched from inside an Atuin-enabled shell session. Clients like editors usually launch it outside of one, in which case the other filter modes still work as normal.
