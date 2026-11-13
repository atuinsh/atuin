<p align="center">
 <picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://github.com/atuinsh/atuin/assets/53315310/13216a1d-1ac0-4c99-b0eb-d88290fe0efd">
  <img alt="Text changing depending on mode. Light: 'So light!' Dark: 'So dark!'" src="https://github.com/atuinsh/atuin/assets/53315310/08bc86d4-a781-4aaa-8d7e-478ae6bcd129" width="55%">
</picture>
</p>

<p align="center">
<em>making your terminal magical</em>
</p>

<hr/>

<p align="center">
  <a href="https://buildkite.com/atuin/atuin"><img src="https://img.shields.io/buildkite/6213f2ad159947780451bdc7fca76c8a8a0be3d3bca66b8a6d/main?style=flat-square&logo=buildkite&logoColor=white" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/v/atuin.svg?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/d/atuin.svg?style=flat-square" /></a>
  <a href="https://github.com/atuinsh/atuin/blob/main/LICENSE"><img src="https://img.shields.io/crates/l/atuin.svg?style=flat-square" /></a>
  <a href="https://docs.atuin.sh"><img src="https://img.shields.io/badge/docs-docs.atuin.sh-blue?style=flat-square&logo=readthedocs&logoColor=white" /></a>
  <a href="https://forum.atuin.sh"><img src="https://img.shields.io/discourse/users?server=https%3A%2F%2Fforum.atuin.sh&style=flat-square&label=forum&logo=discourse&logoColor=white" /></a>
  <a href="https://discord.gg/Fq8bJSKPHh"><img src="https://img.shields.io/discord/954121165239115808?style=flat-square&label=discord&logo=discord&logoColor=white" /></a>
  <a href="https://web.libera.chat/#atuin"><img src="https://img.shields.io/badge/irc-%23atuin-blue?style=flat-square&logo=liberadotchat&logoColor=white" /></a>
  <a rel="me" href="https://hachyderm.io/@atuin"><img src="https://img.shields.io/mastodon/follow/109944632283122560?domain=https%3A%2F%2Fhachyderm.io&style=flat-square&logo=mastodon&logoColor=white" /></a>
  <a href="https://bsky.app/profile/atuin.sh"><img src="https://img.shields.io/bluesky/followers/atuin.sh?style=flat-square&label=follow%20%40atuin.sh&logo=bluesky&logoColor=white" /></a>
  <a href="https://twitter.com/atuinsh"><img src="https://img.shields.io/badge/follow%20%40atuinsh-000000?style=flat-square&logo=x&logoColor=white" /></a>
</p>

Atuin captures everything you do in your terminal, from **all your agentic AI
work**, to all your **terminal history**, enabling you to:

* Let your agents see things you've worked on today, yesterday, a month or a
  year ago.
* Gain context on any work you've ever done in the terminal. If it happened in
  a terminal, Atuin lets you see it!
* Optionally, securely, sync your shell history and AI sessions across all your
  machines.

Atuin is secure by design, even when syncing: everything it syncs is
end-to-end encrypted, and nobody (including you, if you forget the key) can
decrypt it. We're very careful around secrets and avoid storing data with secrets.

# Quick Start

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh
```

Then, **restart your shell**.

## History

Atuin captures everything you do in the terminal, storing it into a SQLite
database. Hitting <kbd>CTRL</kbd><kbd>r</kbd> opens a pretty pane with all
your bash history. Atuin logs your _exit codes_, _working directories_,
_hostnames_, _command duration_ and much much more. You can
hit <kbd>Return</kbd> to execute a command, <kbd>Tab</kbd> to edit any
highlighted command.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://atuin-demos.pages.dev/search-dark.webp">
    <img src="https://atuin-demos.pages.dev/search-light.webp" alt="Searching shell history with ctrl-r, then picking a match" width="100%" />
  </picture>
</p>


Agents can search your history too:

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://atuin-demos.pages.dev/history-dark.webp">
    <img src="https://atuin-demos.pages.dev/history-light.webp" alt="Claude Code finding a command from your shell history through Atuin's MCP server" width="100%" />
  </picture>
</p>

As well as the search UI, `atuin` has a CLI interface for searching commands:

```bash
# search for all successful `make` commands, recorded after 3pm yesterday
atuin search --exit 0 --after "yesterday 3pm" make
```

## Threads

Atuin captures what your agents do. Automatically move your
context and thread history from one machine to another, or from one harness to
another.

With _Atuin Threads_, your agent has access to your full history, and the
built-in Atuin MCP gives your agent context on everything you've ever done.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://atuin-demos.pages.dev/mcp-dark.webp">
    <img src="https://atuin-demos.pages.dev/mcp-light.webp" alt="Claude Code recalling a decision from last week's Codex thread through Atuin's MCP server" width="100%" />
  </picture>
</p>

You can take a session you started on your personal computer in Claude Code,
and `$ atuin resume` it in a remote machine inside Codex.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://atuin-demos.pages.dev/threads-dark.webp">
    <img src="https://atuin-demos.pages.dev/threads-light.webp" alt="A Claude Code thread on a laptop, resumed in Codex over ssh on another machine" width="100%" />
  </picture>
</p>

Similarly to the _history_ index, you can search through your _threads_:

```bash
atuin threads search 'c++'
```

## Sync

Atuin syncs all your AI threads, your shell sessions across all your computers
(even ephemeral VMs!). You can either use the official [Atuin
Service](https://atuin.sh) or you can self-host the sync server. **Everything it
syncs is end-to-end encrypted.**

##

Atuin supports
[<img src="assets/icons/zsh.svg" height="16" align="absmiddle" alt=""> `zsh`](https://www.zsh.org/),
[<img src="assets/icons/bash.svg" height="16" align="absmiddle" alt=""> `bash`](https://www.gnu.org/software/bash/),
[<img src="assets/icons/fish.svg" height="16" align="absmiddle" alt=""> `fish`](https://fishshell.com/),
[<img src="assets/icons/nushell.svg" height="16" align="absmiddle" alt=""> `nushell`](https://www.nushell.sh/),
[<img src="assets/icons/xonsh.svg" height="16" align="absmiddle" alt=""> `xonsh`](https://xon.sh/) and
[<img src="assets/icons/powershell.svg" height="16" align="absmiddle" alt=""> `PowerShell`](https://learn.microsoft.com/en-us/powershell/)
as well as
[<img src="assets/icons/claude-code.svg" height="16" align="absmiddle" alt=""> `Claude Code`](https://claude.com/product/claude-code),
[<img src="assets/icons/codex.svg" height="16" align="absmiddle" alt=""> `Codex`](https://openai.com/codex/),
[<picture><source media="(prefers-color-scheme: dark)" srcset="assets/icons/opencode-dark.svg"><img src="assets/icons/opencode.svg" height="16" align="absmiddle" alt=""></picture> Opencode](https://opencode.ai/),
[<picture><source media="(prefers-color-scheme: dark)" srcset="assets/icons/copilot-dark.svg"><img src="assets/icons/copilot.svg" height="16" align="absmiddle" alt=""></picture> Copilot](https://github.com/features/copilot),
[<picture><source media="(prefers-color-scheme: dark)" srcset="assets/icons/grok-dark.svg"><img src="assets/icons/grok.svg" height="16" align="absmiddle" alt=""></picture> Grok](https://x.ai/build),
[<picture><source media="(prefers-color-scheme: dark)" srcset="assets/icons/hermes-dark.svg"><img src="assets/icons/hermes.svg" height="16" align="absmiddle" alt=""></picture> Hermes](https://hermes-agent.nousresearch.com/),
and many more.

Atuin does so much more and we have [extensive
documentation](https://docs.atuin.sh/)! If you ever need help, reach out to us
through our [communities](#community).

## Related projects

* [Catpuccin for Atuin](https://github.com/catppuccin/atuin)
* [eshell-atuin](https://github.com/SqrtMinusOne/eshell-atuin)
* [mob (tiny creatures in your terminal)](https://github.com/bboynton97/mob)

Please open a PR for any others!

# Community

<p align="center">
  <a href="https://forum.atuin.sh"><img src="https://img.shields.io/discourse/users?server=https%3A%2F%2Fforum.atuin.sh&style=flat-square&label=forum&logo=discourse&logoColor=white" /></a>
  <a href="https://discord.gg/Fq8bJSKPHh"><img src="https://img.shields.io/discord/954121165239115808?style=flat-square&label=discord&logo=discord&logoColor=white" /></a>
  <a href="https://web.libera.chat/#atuin"><img src="https://img.shields.io/badge/irc-%23atuin-blue?style=flat-square&logo=liberadotchat&logoColor=white" /></a>
  <a rel="me" href="https://hachyderm.io/@atuin"><img src="https://img.shields.io/mastodon/follow/109944632283122560?domain=https%3A%2F%2Fhachyderm.io&style=flat-square&logo=mastodon&logoColor=white" /></a>
  <a href="https://bsky.app/profile/atuin.sh"><img src="https://img.shields.io/bluesky/followers/atuin.sh?style=flat-square&label=follow%20%40atuin.sh&logo=bluesky&logoColor=white" /></a>
  <a href="https://twitter.com/atuinsh"><img src="https://img.shields.io/badge/follow%20%40atuinsh-000000?style=flat-square&logo=x&logoColor=white" /></a>
</p>

## Contributors

We are very grateful to all our contributors!

<a href="https://github.com/atuinsh/atuin/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=atuinsh/atuin&max=300&columns=24" />
</a>

_Made with [contrib.rocks](https://contrib.rocks)._
