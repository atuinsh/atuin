# Supported platforms

Atuin runs on a range of shells and operating systems, but not every
combination gets the same level of attention. This page defines our support
tiers and records what works where. It's the single source of truth: other
pages link here instead of keeping their own lists.

"Supported" describes our level of commitment, not merely whether something
runs. A tier 2 shell still works — it just isn't covered by the same testing
and feature guarantees as a tier 1 shell.

## Support tiers

- **Tier 1 — Supported.** Exercised in continuous integration, feature-complete,
  and bugs are prioritized.
- **Tier 2 — Best-effort.** Ships and works, but some features are missing and
  it's largely community-maintained. Not fully covered by continuous
  integration.
- **Experimental.** May change or break between releases. Use at your own risk.

## Shells

| Shell | Tier |
| --- | --- |
| zsh | Tier 1 |
| bash | Tier 1 |
| fish | Tier 1 |
| nushell | Tier 2 |
| xonsh | Tier 2 |
| PowerShell | Tier 2 |

Feature coverage varies by shell:

| Shell | History search | Inline popup | Dotfiles | Atuin AI | pty-proxy |
| --- | :---: | :---: | :---: | :---: | :---: |
| zsh | ✓ | ✓ | ✓ | ✓ | ✓ |
| bash | ✓ | ✓ | ✓ | ✓ | ✓ |
| fish | ✓ | ✓ | ✓ | ✓ | ✓ |
| nushell | ✓ | ✗ | ✗ | ✗ | ✓ |
| xonsh | ✓ | ✗ | ✓ | ✗ | ✗ |
| PowerShell | ✓ | ✗ | ✓ | ✗ | ✗ |

Some gaps depend on the operating system rather than the shell:

- Inline history search isn't available on macOS yet.
- Syntax highlighting (tree-sitter) doesn't build on Windows.

## Operating systems

| Operating system | Tier |
| --- | --- |
| Linux | Tier 1 |
| macOS | Tier 1 |
| Windows | Tier 2 |
| WSL | Tier 2 |
| *BSD | Experimental |

Atuin stores history in SQLite, which struggles on some network filesystems
(such as NFS) and copy-on-write filesystems (such as ZFS). See
[Known issues](known-issues.md). On Windows, install with WinGet — see
[Installation](guide/installation.md).

## Terminals

Some key bindings depend on the terminal, not the shell. Terminals that
implement the
[kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
can report modifier keys, function keys (F1–F24), media keys, and the
super (Command) modifier to Atuin. Terminals without it — including the default
macOS Terminal — are limited to basic key combinations. For details, see
[Advanced key binding](configuration/advanced-key-binding.md).

## AI agents

Atuin records commands run by AI coding agents through their hook systems. See
[AI agent hooks](guide/agent-hooks.md) for setup.

| Agent | Tier |
| --- | --- |
| Claude Code | Tier 1 |
| Codex | Tier 2 |
| Copilot | Tier 2 |
| opencode | Tier 2 |
| pi | Tier 2 |

## Getting help

If something isn't working, [`atuin doctor`](reference/doctor.md) collects the
details we'll ask for. Then open a topic on the
[forum](https://forum.atuin.sh), join our
[Discord](https://discord.gg/Fq8bJSKPHh), or file an
[issue](https://github.com/atuinsh/atuin/issues).
