# Supported platforms

Atuin runs on a range of shells and operating systems, but not every
combination gets the same level of attention. This page defines our support
tiers and records what works where.

## Support tiers

- **Tier 1** — Actively supported by the Atuin team.
- **Tier 2** — Actively supported by the community.

## Shells

Feature coverage varies by shell:

| Tier | Shell | History search | Inline popup | Dotfiles | Atuin AI | pty-proxy |
| :--: | ----- | :---: | :---: | :---: | :---: | :---: |
| 1 | zsh | ✓ | ✓ | ✓ | ✓ | ✓ |
| 1 | bash | ✓ | ✓ | ✓ | ✓ | ✓ |
| 1 | fish | ✓ | ✓ | ✓ | ✓ | ✓ |
| 2 | nushell | ✓ | ✗ | ✗ | ✗ | ✓ |
| 2 | xonsh | ✓ | ✗ | ✓ | ✗ | ✗ |
| 2 | PowerShell | ✓ | ✗ | ✓ | ✗ | ✗ |

## Operating systems and architectures

Atuin runs on a range of operating systems and CPU architectures. The matrix
below shows, for each platform, whether an official prebuilt binary is published
and how far continuous integration (CI) exercises it.

| Tier | Platform | Prebuilt binary | CI | Notes |
| :--: | --- | :--: | :--: | --- |
| 1 | `linux-x86_64` | ✓ | tested | |
| 1 | `linux-arm64` | ✓ | build-only | |
| 1 | `macos-arm64` | ✓ | tested | Replaying a result with ++alt+"&num;"++ isn't available. |
| 1 | `macos-x86_64` | ✓ | build-only | Replaying a result with ++alt+"&num;"++ isn't available. |
| 1 | `windows-x86_64` | ✓ | tested | No syntax highlighting or pty-proxy. |
| 1 | `wsl2-x86_64` | ✓ | tested | |
| 2 | `illumos-x86_64` | ✗ | build-only | Build from source. |
| 2 | Other | ✗ | ✗ | Build from source; untested. Needs a C toolchain. |

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
