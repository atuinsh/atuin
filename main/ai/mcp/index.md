# MCP Server

Atuin ships with a built-in [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) server, giving external AI tools like Claude Code and Cursor access to your shell history. Your agent can look up commands you've run before, check whether they succeeded, and — with [output capture](https://docs.atuin.sh/guide/output-capture/index.md) set up — read what they printed, or search that output for something it remembers seeing.

The server exposes the same history tools that [Atuin AI](https://docs.atuin.sh/ai/introduction/index.md) uses, plus output search. All tools are read-only: nothing can modify or delete your history, and all data stays on your machine.

## Starting the server

The MCP server runs over stdio, so your MCP client starts it for you — there's nothing to keep running in the background. The command is:

```
atuin mcp
```

### Claude Code

```
claude mcp add atuin -- atuin mcp
```

### Cursor, Claude Desktop, and other clients

Most MCP clients accept a JSON configuration like this:

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

If the `atuin` binary isn't on your client's `PATH`, use the full path to the binary instead (for example, `~/.atuin/bin/atuin`).

## Tools

### `atuin_history`

Searches your shell history, using the same fuzzy matching as the search TUI. Each result includes the command, when and where it ran, its exit code, and its duration, along with a history ID that can be passed to `atuin_output`.

Searches can be narrowed down in a few ways:

- **Filter mode**: the same scopes as [interactive search](https://docs.atuin.sh/guide/advanced-usage/index.md) — `global` (the default), `host`, `directory`, `workspace`, or `session`. The `directory` and `workspace` scopes are relative to the directory your MCP client launched the server in, which for most editors is your project directory.
- **Failed commands only**: return only commands that exited with a non-zero exit code.
- **Author**: filter to commands you ran yourself, commands run by AI agents, or commands run by one specific agent. See [AI Agent Hooks](https://docs.atuin.sh/guide/agent-hooks/index.md) for how Atuin records agent-run commands.

History search reads the Atuin database directly, so it works without any extra setup.

### `atuin_output`

Fetches the captured terminal output of a previous command, identified by a history ID from `atuin_history` results. The agent can fetch specific line ranges, so it doesn't need to read a huge log to find the error at the end.

Output capture requires the [daemon](https://docs.atuin.sh/reference/daemon/index.md) and [pty-proxy](https://docs.atuin.sh/reference/pty-proxy/index.md) to be running — see [Capturing Command Output](https://docs.atuin.sh/guide/output-capture/index.md) for setup. Without them, the tool responds with an error explaining that no output is available.

### `atuin_output_search`

Searches the captured output of every command, for when the agent knows *what* was printed but not *which* command printed it — an error message, a version string, a hostname. It's the same search as `atuin output search`: terms are matched as whole words and all of them must appear.

Each result gives the command and its history metadata, followed by the output lines around each match with their line numbers, so the agent can pass the history ID and a line range to `atuin_output` to read more.

Like `atuin_output`, this needs output capture set up and the daemon running.

Session scope

The `session` filter mode only works when the MCP server is launched from inside an Atuin-enabled shell session. Clients like editors usually launch it outside of one, in which case the other filter modes still work as normal.
