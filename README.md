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

- store shell history in a sqlite database
- backup encrypted shell history to the cloud
- the same history across terminals, across session, and across machines
- log exit code, cwd, hostname, session, command duration, etc
- smart interactive history search to replace ctrl-r
- calculate statistics such as "most used command"
- old history file is not replaced

## Documentation

- [Quickstart](#quickstart)
- [Install](#install)
- [Import](docs/import.md)
- [Configuration](docs/config.md)
- [Searching history](docs/search.md)
- [Cloud history sync](docs/sync.md)
- [History stats](docs/stats.md)

## Supported Shells

- zsh
- bash

# Quickstart

```
curl https://raw.githubusercontent.com/ellie/atuin/main/install.sh | bash

atuin register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
atuin import auto
atuin sync
```

## Install

### Script (recommended)

The install script will help you through the setup, ensuring your shell is
properly configured. It will also use one of the below methods, preferring the
system package manager where possible (AUR, homebrew, etc etc).

```
# do not run this as root, root will be asked for if required
curl https://raw.githubusercontent.com/ellie/atuin/main/install.sh | sh
```

### With cargo

It's best to use [rustup](https://rustup.rs/) to get setup with a Rust
toolchain, then you can run:

```
cargo install atuin
```

### AUR

Atuin is available on the [AUR](https://aur.archlinux.org/packages/atuin/)

```
yay -S atuin # or your AUR helper of choice
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

## ...what's with the name?

Atuin is named after "The Great A'Tuin", a giant turtle from Terry Pratchett's
Discworld series of books.
