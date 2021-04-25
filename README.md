<h1 align="center">
  Atuin
</h1>
<em align="center">Magical shell history</em>

<p align="center">
  <a href="https://github.com/ellie/atuin/actions?query=workflow%3ARust"><img src="https://img.shields.io/github/workflow/status/ellie/atuin/Rust?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/v/atuin.svg?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/d/atuin.svg?style=flat-square" /></a>
  <a href="https://github.com/ellie/atuin/blob/main/LICENSE"><img src="https://img.shields.io/crates/l/atuin.svg?style=flat-square" /></a>
</p>
 
- store shell history in a sqlite database
- back up e2e encrypted history to the cloud, and synchronize between machines
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

# Quickstart

```
curl https://github.com/ellie/atuin/blob/main/install.sh | bash

atuin register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
atuin import auto
atuin sync
```

## Install

### AUR

Atuin is available on the [AUR](https://aur.archlinux.org/packages/atuin/)

```
yay -S atuin # or your AUR helper of choice
```

### With cargo

It's best to use [rustup](https://rustup.rs/) to get setup with a Rust
toolchain, then you can run:

```
cargo install atuin
```

### From source

```
git clone https://github.com/ellie/atuin.git
cd atuin
cargo install --path .
```

### Shell plugin

Once the binary is installed, the shell plugin requires installing. Add

```
eval "$(atuin init)"
```

to your `.zshrc`

## ...what's with the name?

Atuin is named after "The Great A'Tuin", a giant turtle from Terry Pratchett's
Discworld series of books.
