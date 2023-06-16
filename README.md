<p align="center">
<img height="250" src="https://user-images.githubusercontent.com/53315310/171035743-53991112-9477-4f3d-8811-5deee40c7879.png"/>
</p>

<p align="center">
<em>magical shell history</em>
</p>

<hr/>

<p align="center">
  <a href="https://github.com/ellie/atuin/actions?query=workflow%3ARust"><img src="https://img.shields.io/github/actions/workflow/status/ellie/atuin/rust.yml?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/v/atuin.svg?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/d/atuin.svg?style=flat-square" /></a>
  <a href="https://github.com/ellie/atuin/blob/main/LICENSE"><img src="https://img.shields.io/crates/l/atuin.svg?style=flat-square" /></a>
  <a href="https://discord.gg/Fq8bJSKPHh"><img src="https://img.shields.io/discord/954121165239115808" /></a>
  <a rel="me" href="https://hachyderm.io/@atuin"><img src="https://img.shields.io/mastodon/follow/109944632283122560?domain=https%3A%2F%2Fhachyderm.io&style=social"/></a>
  <a href="https://twitter.com/atuinsh"><img src="https://img.shields.io/twitter/follow/atuinsh?style=social" /></a>
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
- backup and sync **encrypted** shell history
- the same history across terminals, across sessions, and across machines
- log exit code, cwd, hostname, session, command duration, etc
- calculate statistics such as "most used command"
- old history file is not replaced
- quick-jump to previous items with <kbd>Alt-\<num\></kbd>
- switch filter modes via ctrl-r; search history just from the current session, directory, or globally

## Documentation

- [Quickstart](#quickstart)
- [Install](#install)
- [Import](https://atuin.sh/docs/commands/import)
- [Configuration](https://atuin.sh/docs/config)
- [Searching history](https://atuin.sh/docs/commands/search)
- [Cloud history sync](https://atuin.sh/docs/commands/sync)
- [History stats](https://atuin.sh/docs/commands/stats)
- [Self host Atuin server](https://atuin.sh/docs/self-hosting)
- [Key binding](https://atuin.sh/docs/config/key-binding)
- [Shell completions](https://atuin.sh/docs/commands/shell-completions)

## Supported Shells

- zsh
- bash
- fish
- nushell
 
## Community

Atuin has a community Discord, available [here](https://discord.gg/Fq8bJSKPHh)

# Quickstart
  
## With the default sync server
  
This will sign you up for the default sync server, hosted by me. Everything is end-to-end encrypted, so your secrets are safe!
  
Read more below for offline-only usage, or for hosting your own server.

```
bash <(curl https://raw.githubusercontent.com/ellie/atuin/main/install.sh)

atuin register -u <USERNAME> -e <EMAIL>
atuin import auto
atuin sync
```

Then restart your shell!
  
### Opt-in to activity graph
Alongside the hosted Atuin server, there is also a service which generates activity graphs for your shell history! These are inspired by the GitHub graph.
  
For example, here is mine:
  
![Activity Graph Example](docs/static/img/activity-graph-example.png)

If you wish to get your own, after signing up for the sync server, run this
  
```
curl https://api.atuin.sh/enable -d $(cat ~/.local/share/atuin/session)
```
  
The response includes the URL to your graph. Feel free to share and/or embed this URL, the token is _not_ a secret, and simply prevents user enumeration. 
  
## Offline only (no sync)
  
```
bash <(curl https://raw.githubusercontent.com/ellie/atuin/main/install.sh)
            
atuin import auto
```

By default, Atuin will check for updates. You can [disable update checks by modifying](https://atuin.sh/docs/config/#update_check) `config.toml`.

Then restart your shell!

## Install

### Script (recommended)

The install script will help you through the setup, ensuring your shell is
properly configured. It will also use one of the below methods, preferring the
system package manager where possible (pacman, homebrew, etc etc).

```
# do not run this as root, root will be asked for if required
bash <(curl https://raw.githubusercontent.com/ellie/atuin/main/install.sh)
```

And then follow [the shell setup](#shell-plugin)

### With cargo

It's best to use [rustup](https://rustup.rs/) to get setup with a Rust
toolchain, then you can run:

```
cargo install atuin
```
  
And then follow [the shell setup](#shell-plugin)

### Homebrew

```
brew install atuin
```
  
And then follow [the shell setup](#shell-plugin)
  
### MacPorts

Atuin is also available in [MacPorts](https://ports.macports.org/port/atuin/)  
  
```
sudo port install atuin
```
  
And then follow [the shell setup](#shell-plugin)

### Nix

This repository is a flake, and can be installed using `nix profile`:

```
nix profile install "github:ellie/atuin"
```

Atuin is also available in [nixpkgs](https://github.com/NixOS/nixpkgs):

```
nix-env -f '<nixpkgs>' -iA atuin
```

And then follow [the shell setup](#shell-plugin)
### Pacman

Atuin is available in the Arch Linux [[extra] repository](https://archlinux.org/packages/extra/x86_64/atuin/):

```
pacman -S atuin
```
  
And then follow [the shell setup](#shell-plugin)

### Termux

Atuin is available in the Termux package repository:

```
pkg install atuin
```
  
And then follow [the shell setup](#shell-plugin)

### From source

```
git clone https://github.com/ellie/atuin.git
cd atuin/atuin
cargo install --path .
```
  
And then follow [the shell setup](#shell-plugin)

## Shell plugin

Once the binary is installed, the shell plugin requires installing. If you use
the install script, this should all be done for you! After installing, remember to restart your shell.

### zsh

```
echo 'eval "$(atuin init zsh)"' >> ~/.zshrc
```

#### Zinit

```sh
zinit load ellie/atuin
```

#### Antigen  
  
```sh  
antigen bundle ellie/atuin@main
```

### bash

We need to setup some hooks, so first install bash-preexec:

```
curl https://raw.githubusercontent.com/rcaloras/bash-preexec/master/bash-preexec.sh -o ~/.bash-preexec.sh
echo '[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh' >> ~/.bashrc
```

Then setup Atuin

```
echo 'eval "$(atuin init bash)"' >> ~/.bashrc
```

**PLEASE NOTE**

bash-preexec currently has an issue where it will stop honoring `ignorespace`. While Atuin will ignore commands prefixed with whitespace, they may still end up in your bash history. Please check your configuration! All other shells do not have this issue.

### fish

Add

```
atuin init fish | source
```

to your `is-interactive` block in your `~/.config/fish/config.fish` file
  
### Fig

Install `atuin` shell plugin in zsh, bash, or fish with [Fig](https://fig.io) in one click. 

<a href="https://fig.io/plugins/shell/atuin" target="_blank"><img src="https://fig.io/badges/install-with-fig.svg" /></a>

### Nushell

Run in *Nushell*:

```
mkdir ~/.local/share/atuin/
atuin init nu | save ~/.local/share/atuin/init.nu
```

Add to `config.nu`:

```
source ~/.local/share/atuin/init.nu
```

[English]: ./README.md
[简体中文]: ./docs/zh-CN/README.md
