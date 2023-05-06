---
title: Getting Started
sidebar_position: 1
---

Atuin replaces your existing shell history with a SQLite database, and records
additional context for your commands. Additionally, it provides optional and
_fully encrypted_ synchronisation of your history between machines, via an Atuin
server.

You may use either the server I host, or host your own! Or just don't use sync
at all. As all history sync is encrypted, I couldn't access your data even if
I wanted to. And I **really** don't want to.

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

atuin register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
atuin import auto
atuin sync
```

Then restart your shell!
  
### Opt-in to activity graph
Alongside the hosted Atuin server, there is also a service which generates activity graphs for your shell history! These are inspired by the GitHub graph.
  
For example, here is mine:
  
![Activity Graph Example](/img/activity-graph-example.png)

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

Atuin is available in the Arch Linux [community repository](https://archlinux.org/packages/community/x86_64/atuin/):

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

### fish

Add

```
atuin init fish | source
```

to your `is-interactive` block in your `~/.config/fish/config.fish` file
  
### Fig

Install `atuin` shell plugin in zsh, bash, or fish with [Fig](https://fig.io) in one click. 

<a href="https://fig.io/plugins/other/atuin" target="_blank"><img src="https://fig.io/badges/install-with-fig.svg" /></a>

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
