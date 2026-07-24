# Supported platforms

Atuin runs on a range of shells and operating systems, but not every
combination gets the same level of attention. This page defines our support
tiers and records what works where.

## Support tiers

- **Tier 1 — Supported.** Exercised in CI, feature-complete, and bugs are
  prioritized.
- **Tier 2 — Best-effort.** Ships and works, but some features are missing and
it's largely community-maintained. Not fully covered by continuous integration.
- **Experimental.** May change or break between releases. Use at your own risk.

## Shells

Feature coverage varies by shell:

| Shell | History search | Inline popup | Dotfiles | Atuin AI | pty-proxy |
| ----- | :---: | :---: | :---: | :---: | :---: |
| zsh (T1) | ✓ | ✓ | ✓ | ✓ | ✓ |
| bash (T1) | ✓ | ✓ | ✓ | ✓ | ✓ |
| fish (T1) | ✓ | ✓ | ✓ | ✓ | ✓ |
| nushell (T2) | ✓ | ✗ | ✗ | ✗ | ✓ |
| xonsh (T2) | ✓ | ✗ | ✓ | ✗ | ✗ |
| PowerShell (T2) | ✓ | ✗ | ✓ | ✗ | ✗ |

## Operating systems and architectures

Atuin runs on a range of operating systems and CPU architectures. The matrix
below shows, for each platform, whether an official prebuilt binary is published
and how far continuous integration (CI) exercises it.

| Platform | Prebuilt binary | CI | Notes |
| --- | :--: | :--: | --- |
| `linux-x86_64` | ✓ | tested | |
| `linux-arm64` | ✓ | build-only | |
| `macos-arm64` | ✓ | tested | Replaying a result with ++alt+"&num;"++ isn't available. |
| `windows-x86_64` | ✓ | tested | No syntax highlighting or pty-proxy. |
| `wsl2-x86_64` | ✓ | tested | |
| `illumos-x86_64` | ✗ | build-only | Build from source. |
| Other (riscv64, …) | ✗ | ✗ | Build from source. May not compile: TLS (`ring`) supports only x86_64, x86, arm, and aarch64. |

- **Prebuilt binary** — an official binary is published through the install
  script and GitHub releases. Linux binaries are provided for both glibc and
  musl.
- **CI** — _tested_: unit tests run in CI; _build-only_: compiled and checked,
  but tests aren't run; _✗_: not built in CI.

!!! warning

    Atuin supports running on all filesystems. ZFS and network filesystems are
    treated as _Experimental_. For more info, see [Known
    issues](known-issues.md).

## Terminals

Some key bindings depend on the terminal, not the shell. Terminals that
implement the [kitty keyboard
protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/) can report
modifier keys, function keys (++"Fn1"++–++"Fn24"++), media keys, and the super
(Command) modifier to Atuin. Terminals without it — including the default macOS
Terminal — are limited to basic key combinations. For details, see [Advanced
key binding](configuration/advanced-key-binding.md).

## AI agents

Atuin records commands run by AI coding agents through their hook systems. See
[AI agent hooks](guide/agent-hooks.md) for setup. The supported agents are
_Claude Code_, _Codex_, _Copilot_, _opencode_ and _pi_.

## Getting help

If something isn't working, [`atuin doctor`](reference/doctor.md) collects the
details we'll ask for. Then open a topic on the
[forum](https://forum.atuin.sh), join our
[Discord](https://discord.gg/Fq8bJSKPHh), or file an
[issue](https://github.com/atuinsh/atuin/issues).
