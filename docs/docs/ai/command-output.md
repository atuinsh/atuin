# Reading Command Output

Atuin AI can read the output of commands you've run. Ask "why did that fail?" and it can look at the actual error message, rather than guessing from the command alone.

Atuin doesn't capture output by default. See [Capturing Command Output](../guide/output-capture.md) for how capture works, what it keeps, and how to control it.

## Setup

Turn on output capture, which sets up the [daemon](../reference/daemon.md) and [pty-proxy](../reference/pty-proxy.md) it needs:

```shell
atuin config enable output-capture
```

Then open a new terminal. To set it up by hand instead, see [Setting it up](../guide/output-capture.md#setting-it-up).

To try it out, run a command that fails, then press ++question++ and ask Atuin AI why it failed. It will ask permission to use the `AtuinOutput` tool, then read the output and answer.

## How it works

pty-proxy sits between your terminal and your shell, and uses your shell's prompt markers to work out where each command's output starts and ends. It then sends each captured command to the daemon, which stores it on disk alongside its Atuin history ID. When Atuin AI wants to see what a command printed, it asks the daemon for the output by history ID, and can ask for just the lines it needs.

## Privacy

Captured output stays on your machine, and Atuin sends nothing to the LLM until the LLM requests the output of a specific command. By default, Atuin AI asks your permission first.

To keep a command's output away from the AI, keep it out of the store: add the command to [`command_filter`](../configuration/config.md#command_filter) to keep it in your history without its output. See [Privacy](../guide/output-capture.md#privacy) for what else is filtered or redacted.

## Permissions

Output retrieval is controlled by the `AtuinOutput` permission rule — see [Tools & Permissions](./tools-permissions.md). To let Atuin AI read command output without asking every time:

```toml
[permissions]

allow = ["AtuinOutput"]
```

To turn the capability off entirely, set `ai.capabilities.enable_history_output` to `false` in your Atuin config (see the [settings documentation](./settings.md#capabilities)).

## Reading output from other AI tools

Captured output isn't limited to Atuin AI: external tools like Claude Code and Cursor can read it too, via Atuin's [MCP server](./mcp.md).
