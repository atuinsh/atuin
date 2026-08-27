# Supported platforms

Atuin runs on a range of shells and operating systems, but not every
combination gets the same level of attention. This page defines our support
tiers and records what works where.

## Support tiers

- **Tier 1** — Actively supported by the Atuin team.
- **Tier 2** — Supported by the community. Complex issues under this support
               level don't block a release; support is best-effort.

## Shells

Feature coverage varies by shell:

<div class="support-matrix">
<table>
  <thead>
    <tr>
      <th class="tier">Tier</th><th>Shell</th><th>History search</th><th>Inline popup</th>
      <th>Dotfiles</th><th>Atuin AI</th><th>pty-proxy</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td rowspan="3" class="tier"><strong>1</strong></td>
      <td>zsh</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td>
    </tr>
    <tr><td>bash</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td></tr>
    <tr><td>fish</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td><td class="support-yes">✓</td></tr>
    <tr>
      <td rowspan="3" class="tier"><strong>2</strong></td>
      <td>nushell</td><td class="support-yes">✓</td><td class="support-no">✗</td><td class="support-no">✗</td><td class="support-no">✗</td><td class="support-yes">✓</td>
    </tr>
    <tr><td>xonsh</td><td class="support-yes">✓</td><td class="support-no">✗</td><td class="support-yes">✓</td><td class="support-no">✗</td><td class="support-no">✗</td></tr>
    <tr><td>PowerShell</td><td class="support-yes">✓</td><td class="support-no">✗</td><td class="support-yes">✓</td><td class="support-no">✗</td><td class="support-no">✗</td></tr>
  </tbody>
</table>
</div>

## Operating systems and architectures

Atuin runs on a range of operating systems and CPU architectures. The matrix
below shows, for each platform, whether an official prebuilt binary is published
and how far continuous integration (CI) exercises it.

<div class="support-matrix">
<table>
  <thead>
    <tr>
      <th class="tier">Tier</th><th>OS</th><th>Arch</th><th>Prebuilt</th><th>CI</th><th>Notes</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td rowspan="6" class="tier"><strong>1</strong></td>
      <td rowspan="2"><code>Linux</code></td>
      <td><code>x86_64</code></td><td class="support-yes">✓</td><td>tested</td><td></td>
    </tr>
    <tr><td><code>arm64</code></td><td class="support-yes">✓</td><td>build-only</td><td></td></tr>
    <tr>
      <td rowspan="2"><code>macOS</code></td>
      <td><code>arm64</code></td><td class="support-yes">✓</td><td>tested</td>
      <td rowspan="2">Replaying a result with <kbd>alt</kbd>+<kbd>#</kbd> isn't available.</td>
    </tr>
    <tr><td><code>x86_64</code></td><td class="support-yes">✓</td><td>build-only</td></tr>
    <tr>
      <td><code>Windows</code></td><td><code>x86_64</code></td><td class="support-yes">✓</td><td>tested</td>
      <td>No syntax highlighting or <code>pty-proxy</code>.</td>
    </tr>
    <tr><td><code>WSL-2</code></td><td><code>x86_64</code></td><td class="support-yes">✓</td><td>tested</td><td></td></tr>
    <tr>
      <td rowspan="2" class="tier"><strong>2</strong></td>
      <td><code>Illumos</code></td><td><code>x86_64</code></td><td class="support-no">✗</td><td>build-only</td>
      <td></td>
    </tr>
    <tr>
      <td><em>Other</em></td><td>—</td><td class="support-no">✗</td><td class="support-no">✗</td>
      <td>Build from source.</td>
    </tr>
  </tbody>
</table>
</div>

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
