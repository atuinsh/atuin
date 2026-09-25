# Capturing Command Output

Atuin can store what each command printed next to its history entry. You can
then read it back in the search UI, search across it with `atuin output
search`, and let [Atuin AI](../ai/command-output.md) or other AI tools read
what a command actually did.

Output capture is off by default. It needs [pty-proxy](../reference/pty-proxy.md),
which sees what your terminal displays, and the [daemon](../reference/daemon.md),
which stores it. pty-proxy supports bash, zsh, fish, and Nushell, and doesn't
run on Windows; see [Supported platforms](../support.md).

## Setting it up

```shell
atuin config enable output-capture
```

This turns on the daemon (with `autostart`) if it isn't on already, enables
pty-proxy and `[output]`, and restarts the daemon. Open a new shell afterward.

To do the same by hand, add to your config file:

```toml
[daemon]
enabled = true
autostart = true

[pty_proxy]
enabled = true

[output]
enabled = true
```

If you manage the daemon yourself (systemd, `launchd`), restart it after
changing `[output]`. See [`[pty_proxy]`](../reference/pty-proxy.md#initialization)
for other ways to start pty-proxy.

## Reading captured output

### In the search UI

Press **Ctrl + o** in search to open the [inspector](../configuration/key-binding.md#inspector),
pick a run, and press **Enter** (or **o**) to open its output. Colours and
formatting are kept.

### From the command line

[`atuin output search`](../reference/output.md) searches the output of every
captured command:

```shell
atuin output search connection refused
```

### From AI tools

- [Atuin AI](../ai/command-output.md) reads a command's output when you ask it
  why something failed.
- Claude Code, Cursor, and other agents can read and search it through Atuin's
  [MCP server](../ai/mcp.md).

## What gets captured

Atuin stores the output as your terminal displayed it, not the raw bytes the
command wrote: a progress bar that redraws itself is stored as it finally
looked.

Some output is never stored:

- **Full-screen programs** such as `vim`, `less`, and `htop`. Anything drawn on
  the terminal's alternate screen is skipped.
- **Commands Atuin doesn't record.** Output belongs to a history entry, so a
  command typed with a leading space, one excluded by
  [`history_filter`](../configuration/config.md#history_filter) or
  [`cwd_filter`](../configuration/config.md#cwd_filter), or a failing command
  with [`store_failed = false`](../configuration/config.md#store_failed) has
  none.
- **Commands in [`command_filter`](../configuration/config.md#command_filter).**
  These stay in your history; only their output is dropped.
- **Atuin's own credential commands**: `atuin key`, `atuin login`,
  `atuin register`, and `atuin account change-password`.

When a command prints more than
[`max_output_size`](../configuration/config.md#max_output_size) (1MB by
default), Atuin keeps the start and the end, half each, and drops the middle,
recording that it's missing.

## Privacy

- Recognised credentials in the output (API keys, tokens, and the like) are
  replaced with `****` before storage, while
  [`secrets_filter`](../configuration/config.md#secrets_filter) is on (the
  default).
- To keep a command in your history but never store its output, add it to
  [`command_filter`](../configuration/config.md#command_filter). See
  [Excluding commands](excluding-commands.md#keep-the-command-drop-its-output-command_filter).
- Captured output stays on your machine. It isn't synced, even with
  [sync](sync.md) set up.
- Atuin AI only sends output to the LLM when it asks for a specific command's
  output, and asks your permission first by default. See [Reading Command
  Output](../ai/command-output.md#permissions).

## Storage and retention

Output is stored on disk in the `output-capture` directory of Atuin's data
directory (`~/.local/share/atuin/output-capture` by default), so it survives
daemon restarts. Don't edit that directory by hand.

- **Disk budget.** Output may use up to
  [`max_disk_usage`](../configuration/config.md#max_disk_usage), 10% of the disk
  by default. The daemon checks about once a minute; past 95% of the budget, it
  deletes the oldest output until usage is back to 90%.
- **Deleting history deletes output.** Deleting an entry from the search UI,
  with `atuin search --delete`, `atuin history prune`, or `atuin history dedup`
  also deletes its output.
- **Turning it off.** Set `[output] enabled = false`, then restart the daemon
  and your shell. Output already stored stays on disk; to remove it, stop the
  daemon and delete the `output-capture` directory.

## When changes take effect

| Setting | Takes effect |
| ------- | ------------ |
| `command_filter` | On the next command; the daemon reloads its config by itself |
| `enabled`, `max_disk_usage` | After `atuin daemon restart` and a new shell |
| `max_output_size` | In a new shell, when pty-proxy starts |

All settings are listed in the [configuration
reference](../configuration/config.md#output-capture).
