# MCP Server

Atuin ships with a built-in [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) server, giving external AI tools like Claude Code and Cursor access to your shell history. Your agent can look up commands you've run before, check whether they succeeded, and — with [output capture](https://docs.atuin.sh/ai/command-output/index.md) set up — read what they printed.

The server exposes the same history tools that [Atuin AI](https://docs.atuin.sh/ai/introduction/index.md) uses. Both tools are read-only: nothing can modify or delete your history, and all data stays on your machine.

## Starting the server

The MCP server runs over stdio, so your MCP client starts it for you — there's nothing to keep running in the background. The command is simply:

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

If the `atuin` binary is not on your client's `PATH`, use the full path to the binary instead (e.g. `~/.atuin/bin/atuin`).

## Tools

### `atuin_history`

Searches your shell history, using the same fuzzy matching as the search TUI. Each result includes the command, when and where it ran, its exit code, and its duration, along with a history ID that can be passed to `atuin_output`.

Searches can be narrowed down in a few ways:

- **Filter mode**: the same scopes as [interactive search](https://docs.atuin.sh/guide/advanced-usage/index.md) — `global`, `host`, `directory`, `workspace`, or `session`. The `directory` and `workspace` scopes are relative to the directory the server was launched in, which for most editors is your project directory.
- **Failed commands only**: return only commands that exited with a non-zero exit code.
- **Author**: filter to commands you ran yourself, commands run by AI agents, or commands run by one specific agent. See [AI Agent Hooks](https://docs.atuin.sh/guide/agent-hooks/index.md) for how agent-run commands are recorded.

History search reads the Atuin database directly, so it works without any extra setup.

### `atuin_output`

Fetches the captured terminal output of a previous command, identified by a history ID from `atuin_history` results. The agent can fetch specific line ranges, so it doesn't need to read a huge log to find the error at the end.

Output capture requires the [daemon](https://docs.atuin.sh/reference/daemon/index.md) and [pty-proxy](https://docs.atuin.sh/reference/pty-proxy/index.md) to be running — see [Reading Command Output](https://docs.atuin.sh/ai/command-output/index.md) for setup. Without them, the tool responds with an error explaining that no output is available.

Session scope

The `session` filter mode only works when the MCP server is launched from inside an Atuin-enabled shell session. Clients like editors usually launch it outside of one, in which case the other filter modes still work as normal.
