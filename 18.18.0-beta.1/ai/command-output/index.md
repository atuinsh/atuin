# Reading Command Output

Atuin AI can read the output of commands you've run. Ask "why did that fail?" and it can look at the actual error message, rather than guessing from the command alone.

Atuin doesn't capture output by default — it needs two pieces set up: the [daemon](https://docs.atuin.sh/reference/daemon/index.md), which stores recent output in memory, and [pty-proxy](https://docs.atuin.sh/reference/pty-proxy/index.md), which captures it from your terminal.

## Setup

### 1. Enable the daemon

Add the following to your Atuin config file (`~/.config/atuin/config.toml` by default):

```
[daemon]
enabled = true
autostart = true
```

With `autostart = true`, Atuin starts and manages the daemon for you. If you'd rather run it yourself (for example via systemd), see the [daemon documentation](https://docs.atuin.sh/reference/daemon/index.md).

### 2. Enable pty-proxy

Add the pty-proxy init line to your shell's init script, as high in the file as possible, *before* your normal `atuin init` call:

```
eval "$(atuin pty-proxy init zsh)"
```

```
eval "$(atuin pty-proxy init bash)"
```

Add

```
atuin pty-proxy init fish | source
```

to your `is-interactive` block in your `~/.config/fish/config.fish` file

Run in *Nushell*:

```
mkdir ~/.local/share/atuin/
atuin pty-proxy init nu | save -f ~/.local/share/atuin/pty-proxy-init.nu
```

Add to `config.nu`, **before** the regular `atuin init`:

```
source ~/.local/share/atuin/pty-proxy-init.nu
```

See the [pty-proxy documentation](https://docs.atuin.sh/reference/pty-proxy/index.md) for more detail, including what to do if `atuin` is not on your `PATH` when your shell starts.

### 3. Restart your shell

Open a new terminal (or re-source your shell config). From now on, the output of every command you run in that session is captured and available to the AI.

To try it out, run a command that fails, then press `?` and ask Atuin AI why it failed. It will ask permission to use the `AtuinOutput` tool, then read the output and answer.

## How it works

pty-proxy sits between your terminal and your shell, and uses your shell's prompt markers to work out where each command's output starts and ends. Each captured command is sent to the daemon, which keeps it in memory alongside its Atuin history ID. When Atuin AI wants to see what a command printed, it asks the daemon for the output by history ID.

## Privacy and retention

Captured output is stored in memory, on your machine:

- The daemon keeps up to 1MB of output per command, and the most recent 128 commands (up to 32MB of output) per shell session.
- Output is lost when the daemon stops. Only commands captured while the daemon was running are available.

Nothing is sent to the LLM until it requests the output of a specific command, and by default Atuin AI asks your permission first.

## Permissions

Output retrieval is controlled by the `AtuinOutput` permission rule — see [Tools & Permissions](https://docs.atuin.sh/ai/tools-permissions/index.md). To let Atuin AI read command output without asking every time:

```
[permissions]

allow = ["AtuinOutput"]
```

To turn the capability off entirely, set `ai.capabilities.enable_history_output` to `false` in your Atuin config (see the [settings documentation](https://docs.atuin.sh/ai/settings/#capabilities)).

## Reading output from other AI tools

Captured output isn't limited to Atuin AI: external tools like Claude Code and Cursor can read it too, via Atuin's [MCP server](https://docs.atuin.sh/ai/mcp/index.md).
