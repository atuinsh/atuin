# Supported platforms

Atuin runs on a range of shells and operating systems, but not every combination gets the same level of attention. This page defines our support tiers and records what works where.

## Support tiers

- **Tier 1** — Actively supported by the Atuin team.
- **Tier 2** — Supported by the community. Complex issues under this support level don't block a release; support is best-effort.

## Shells

Feature coverage varies by shell:

| Tier       | Shell   | History search | Inline popup | Dotfiles | Atuin AI | pty-proxy |
| ---------- | ------- | -------------- | ------------ | -------- | -------- | --------- |
| **1**      | zsh     | ✓              | ✓            | ✓        | ✓        | ✓         |
| bash       | ✓       | ✓              | ✓            | ✓        | ✓        |           |
| fish       | ✓       | ✓              | ✓            | ✓        | ✓        |           |
| **2**      | nushell | ✓              | ✗            | ✗        | ✗        | ✓         |
| xonsh      | ✓       | ✗              | ✓            | ✗        | ✗        |           |
| PowerShell | ✓       | ✗              | ✓            | ✗        | ✗        |           |

## Operating systems and architectures

Atuin runs on a range of operating systems and CPU architectures. The matrix below shows, for each platform, whether an official prebuilt binary is published and how far continuous integration (CI) exercises it.

| Tier      | OS        | Arch       | Prebuilt | CI                                                 | Notes |
| --------- | --------- | ---------- | -------- | -------------------------------------------------- | ----- |
| **1**     | `Linux`   | `x86_64`   | ✓        | tested                                             |       |
| `arm64`   | ✓         | build-only |          |                                                    |       |
| `macOS`   | `arm64`   | ✓          | tested   | Replaying a result with `alt`+`#` isn't available. |       |
| `x86_64`  | ✓         | build-only |          |                                                    |       |
| `Windows` | `x86_64`  | ✓          | tested   | No syntax highlighting or `pty-proxy`.             |       |
| `WSL-2`   | `x86_64`  | ✓          | tested   |                                                    |       |
| **2**     | `Illumos` | `x86_64`   | ✗        | build-only                                         |       |
| *Other*   | —         | ✗          | ✗        | Build from source.                                 |       |

## Terminals

Some key bindings depend on the terminal, not the shell. Terminals that implement the [kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/) can report modifier keys, function keys (`Fn1`–`Fn24`), media keys, and the super (Command) modifier to Atuin. Terminals without it — including the default macOS Terminal — are limited to basic key combinations. For details, see [Advanced key binding](https://docs.atuin.sh/configuration/advanced-key-binding/index.md).

## AI agents

Atuin records commands run by AI coding agents through their hook systems. See [AI agent hooks](https://docs.atuin.sh/guide/agent-hooks/index.md) for setup. The supported agents are *Claude Code*, *Codex*, *Copilot*, *opencode* and *pi*.

## Getting help

If something isn't working, [`atuin doctor`](https://docs.atuin.sh/reference/doctor/index.md) collects the details we'll ask for. Then open a topic on the [forum](https://forum.atuin.sh), join our [Discord](https://discord.gg/Fq8bJSKPHh), or file an [issue](https://github.com/atuinsh/atuin/issues).
