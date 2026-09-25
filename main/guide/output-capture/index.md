# Capturing Command Output

Atuin can store what each command printed next to its history entry. You can then read it back in the inspector UI, search across it with `atuin output search`, and let [Atuin AI](https://docs.atuin.sh/ai/command-output/index.md) or other AI tools read what a command actually did.

Output capture is off by default. It needs [pty-proxy](https://docs.atuin.sh/reference/pty-proxy/index.md), which sees what your terminal displays, and the [daemon](https://docs.atuin.sh/reference/daemon/index.md), which stores it.

## Setting it up

```
atuin config enable output-capture
```

This turns on the daemon (with `autostart`) if it isn't on already, enables pty-proxy and `[output]`, and restarts the daemon. Open a new shell afterward.

To do the same by hand, add to your config file:

```
[daemon]
enabled = true
autostart = true

[pty_proxy]
enabled = true

[output]
enabled = true
```

If you manage the daemon yourself (systemd, `launchd`), restart it after changing `[output]`. See [`[pty_proxy]`](https://docs.atuin.sh/reference/pty-proxy/#initialization) for other ways to start pty-proxy.

## Reading captured output

### In the search UI

Press `Ctrl`+`O` in search to open the [inspector](https://docs.atuin.sh/configuration/key-binding/#inspector), pick a run, and press `Enter` (or `O`) to open its output. Colours and formatting are kept.

### From the command line

[`atuin output search`](https://docs.atuin.sh/reference/output/index.md) searches the output of every captured command:

```
atuin output search connection refused
```

### From AI tools

- [Atuin AI](https://docs.atuin.sh/ai/command-output/index.md) reads a command's output when you ask it why something failed.
- Claude Code, Cursor, and other agents can read and search it through Atuin's [MCP server](https://docs.atuin.sh/ai/mcp/index.md).

## What gets captured

Atuin stores the output as your terminal displayed it at the end of the command. If you had progress bars on similar animated effects, only the "last frame" is recorded.

Some output is never stored:

- **Full-screen programs** such as `vim`, `less`, and `htop`. Anything drawn on the terminal's alternate screen is skipped.
- **Commands Atuin doesn't record.** Output belongs to a history entry, so a command typed with a leading space, one excluded by [`history_filter`](https://docs.atuin.sh/configuration/config/#history_filter) or [`cwd_filter`](https://docs.atuin.sh/configuration/config/#cwd_filter), or a failing command with [`store_failed = false`](https://docs.atuin.sh/configuration/config/#store_failed) has none.
- **Commands in [`command_filter`](https://docs.atuin.sh/configuration/config/#command_filter).** These stay in your history; only their output is dropped.
- **Atuin's own credential commands**: `atuin key`, `atuin login`, `atuin register`, and `atuin account change-password`.

When a command prints more than [`max_output_size`](https://docs.atuin.sh/configuration/config/#max_output_size) (1MB by default), Atuin keeps the start and the end, half each, and drops the middle, recording that it's missing.

## Privacy

- Recognised credentials in the output (API keys, tokens, and the like) are replaced with `****` before storage, while [`secrets_filter`](https://docs.atuin.sh/configuration/config/#secrets_filter) is on (the default). This is best-effort: it only knows common formats, and misses a credential that colour codes split apart.
- For a command that prints secrets, don't rely on redaction: add it to [`command_filter`](https://docs.atuin.sh/configuration/config/#command_filter) to keep it in your history but never store its output. See [Excluding commands](https://docs.atuin.sh/guide/excluding-commands/#keep-the-command-drop-its-output-command_filter).
- Captured output stays on your machine. **It isn't synced**, even with [sync](https://docs.atuin.sh/guide/sync/index.md) set up. We're actively working on supporting this.
- Atuin AI only sends output to the LLM when it asks for a specific command's output, and asks your permission first by default. See [Reading Command Output](https://docs.atuin.sh/ai/command-output/#permissions).

## Storage and retention

Output is stored on disk in the `output-capture` directory of Atuin's data directory (`~/.local/share/atuin/output-capture` by default), so it survives daemon restarts. Don't edit that directory by hand.

- **Disk budget.** Output may use up to [`max_disk_usage`](https://docs.atuin.sh/configuration/config/#max_disk_usage), 10% of the disk by default. The daemon checks about once a minute; past 95% of the budget, it deletes the oldest output until usage is back to 90%.
- **Deleting history deletes output.** Deleting an entry from the search UI, with `atuin search --delete`, `atuin history prune`, or `atuin history dedup` also deletes its output.
- **Turning it off.** Set `[output] enabled = false`. The daemon stops storing output right away; open a new shell so pty-proxy stops capturing it too. Output already stored stays on disk; to remove it, stop the daemon and delete the `output-capture` directory.

## When changes take effect

| Setting                            | Takes effect                                                 |
| ---------------------------------- | ------------------------------------------------------------ |
| `command_filter`                   | On the next command; the daemon reloads its config by itself |
| `enabled = false`                  | At once for storage; in a new shell for capturing            |
| `enabled = true`, `max_disk_usage` | After `atuin daemon restart` and a new shell                 |
| `max_output_size`                  | In a new shell, when pty-proxy starts                        |

All settings are listed in the [configuration reference](https://docs.atuin.sh/configuration/config/#output-capture).
