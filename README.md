<h1 align="center">
  Atuin
</h1>

<p align="center">
<em>magical shell history</em>
</p>

<p align="center">
  <a href="https://github.com/ellie/atuin/actions?query=workflow%3ARust"><img src="https://img.shields.io/github/workflow/status/ellie/atuin/Rust?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/v/atuin.svg?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/d/atuin.svg?style=flat-square" /></a>
  <a href="https://github.com/ellie/atuin/blob/main/LICENSE"><img src="https://img.shields.io/crates/l/atuin.svg?style=flat-square" /></a>
</p>

<p align="center">
  <img src="demo.gif" alt="animated" width="80%" />
</p>

<p align="center">
<em>exit code, duration, time and command shown</em>
</p>

Atuin replaces your existing shell history with a SQLite database, and records
additional context for your commands. Additionally, it provides optional and
_fully encrypted_ synchronisation of your history between machines, via an Atuin
server.

As well as the search UI, it can do things like this:

```
# search for all successful `make` commands, recorded after 3pm yesterday
atuin search --exit 0 --after "yesterday 3pm" make
```

You may use either the server I host, or host your own! Or just don't use sync
at all. As all history sync is encrypted, I couldn't access your data even if
I wanted to. And I **really** don't want to.

## Features

- rebind `up` and `ctrl-r` with a full screen history search UI
- store shell history in a sqlite database
- backup and sync **encrypted** shell history
- the same history across terminals, across sessions, and across machines
- log exit code, cwd, hostname, session, command duration, etc
- calculate statistics such as "most used command"
- old history file is not replaced
- quick-jump to previous items with <kbd>Alt-\<num\></kbd>

## Documentation

- [Quickstart](#quickstart)
- [Install](#install)
- [Import](docs/import.md)
- [Configuration](docs/config.md)
- [Searching history](docs/search.md)
- [Cloud history sync](docs/sync.md)
- [History stats](docs/stats.md)
- [Running your own server](docs/server.md)
- [Key binding](docs/key-binding.md)

## Supported Shells

- zsh
- bash

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
  
## Offline only (no sync)
  
```
bash <(curl https://raw.githubusercontent.com/ellie/atuin/main/install.sh)
            
atuin import auto
```

## Install

### Script (recommended)

The install script will help you through the setup, ensuring your shell is
properly configured. It will also use one of the below methods, preferring the
system package manager where possible (pacman, homebrew, etc etc).

```
# do not run this as root, root will be asked for if required
bash <(curl https://raw.githubusercontent.com/ellie/atuin/main/install.sh)
```

### With cargo

It's best to use [rustup](https://rustup.rs/) to get setup with a Rust
toolchain, then you can run:

```
cargo install atuin
```

### Homebrew

```
brew install atuin
```

### Pacman

Atuin is available in the Arch Linux [community repository](https://archlinux.org/packages/community/x86_64/atuin/):

```
pacman -S atuin
```

### From source

```
git clone https://github.com/ellie/atuin.git
cd atuin
cargo install --path .
```

## Shell plugin

Once the binary is installed, the shell plugin requires installing. If you use
the install script, this should all be done for you!

### zsh

```
echo 'eval "$(atuin init zsh)"' >> ~/.zshrc
```

Or using a plugin manager:

```
zinit load ellie/atuin
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

## ...what's with the name?

Atuin is named after "The Great A'Tuin", a giant turtle from Terry Pratchett's
Discworld series of books.
