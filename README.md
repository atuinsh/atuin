<p align="center">
 <picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://github.com/atuinsh/atuin/assets/53315310/13216a1d-1ac0-4c99-b0eb-d88290fe0efd">
  <img alt="Text changing depending on mode. Light: 'So light!' Dark: 'So dark!'" src="https://github.com/atuinsh/atuin/assets/53315310/08bc86d4-a781-4aaa-8d7e-478ae6bcd129">
</picture>
</p>

<p align="center">
<em>magical shell history</em>
</p>

<hr/>

<p align="center">
  <a href="https://github.com/atuinsh/atuin/actions?query=workflow%3ARust"><img src="https://img.shields.io/github/actions/workflow/status/atuinsh/atuin/rust.yml?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/v/atuin.svg?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/d/atuin.svg?style=flat-square" /></a>
  <a href="https://github.com/atuinsh/atuin/blob/main/LICENSE"><img src="https://img.shields.io/crates/l/atuin.svg?style=flat-square" /></a>
  <a href="https://discord.gg/Fq8bJSKPHh"><img src="https://img.shields.io/discord/954121165239115808" /></a>
  <a rel="me" href="https://hachyderm.io/@atuin"><img src="https://img.shields.io/mastodon/follow/109944632283122560?domain=https%3A%2F%2Fhachyderm.io&style=social"/></a>
  <a href="https://twitter.com/atuinsh"><img src="https://img.shields.io/twitter/follow/atuinsh?style=social" /></a>
  <a href="https://actuated.dev/"><img alt="Arm CI sponsored by Actuated" src="https://docs.actuated.dev/images/actuated-badge.png" width="120px"></img></a>
</p>


[English] | [简体中文]


Atuin replaces your existing shell history with a SQLite database, and records
additional context for your commands. Additionally, it provides optional and
_fully encrypted_ synchronisation of your history between machines, via an Atuin
server.




<p align="center">
  <img src="demo.gif" alt="animated" width="80%" />
</p>

<p align="center">
<em>exit code, duration, time and command shown</em>
</p>





As well as the search UI, it can do things like this:

```
# search for all successful `make` commands, recorded after 3pm yesterday
atuin search --exit 0 --after "yesterday 3pm" make
```

You may use either the server I host, or host your own! Or just don't use sync
at all. As all history sync is encrypted, I couldn't access your data even if
I wanted to. And I **really** don't want to.

## Features

- rebind `ctrl-r` and `up` (configurable) to a full screen history search UI
- store shell history in a sqlite database
- back up and sync **encrypted** shell history
- the same history across terminals, across sessions, and across machines
- log exit code, cwd, hostname, session, command duration, etc
- calculate statistics such as "most used command"
- old history file is not replaced
- quick-jump to previous items with <kbd>Alt-\<num\></kbd>
- switch filter modes via ctrl-r; search history just from the current session, directory, or globally
- enter to execute a command, tab to edit

## Documentation

- [Quickstart](#quickstart)
- [Install](https://docs.atuin.sh/guide/installation/)
- [Setting up sync](https://docs.atuin.sh/guide/sync/)
- [Import history](https://docs.atuin.sh/guide/import/)
- [Basic usage](https://docs.atuin.sh/guide/basic-usage/)
## Supported Shells

- zsh
- bash
- fish
- nushell
- xonsh

## Community

### Forum

Atuin has a community forum, please ask here for help and support: https://forum.atuin.sh/

### Discord

Atuin also has a community Discord, available [here](https://discord.gg/jR3tfchVvW)

# Quickstart

This will sign you up for the Atuin Cloud sync server. Everything is end-to-end encrypted, so your secrets are safe!

Read more in the [docs](https://docs.atuin.sh) for an offline setup, self hosted server, and more.

```
curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh

atuin register -u <USERNAME> -e <EMAIL>
atuin import auto
atuin sync
```

Then restart your shell!

> [!NOTE]
>
> **For Bash users**: The above sets up `bash-preexec` for necessary hooks, but
> `bash-preexec` has limitations.  For details, please see the
> [Bash](https://docs.atuin.sh/guide/installation/#installing-the-shell-plugin)
> section of the shell plugin documentation.

# Security

If you find any security issues, we'd appreciate it if you could alert ellie@atuin.sh

# Contributors

<a href="https://github.com/atuinsh/atuin/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=atuinsh/atuin&max=300" />
</a>

Made with [contrib.rocks](https://contrib.rocks).

[English]: ./README.md
[简体中文]: ./docs/zh-CN/README.md
