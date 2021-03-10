<h1 align="center">
  A'tuin
</h1>
<blockquote align="center">
  Through the fathomless deeps of space swims the star turtle Great A’Tuin, bearing on its back the four giant elephants who carry on their shoulders the mass of the Discworld.
 </blockquote>

<p align="center">
  <a href="https://github.com/ellie/atuin/actions?query=workflow%3ARust"><img src="https://img.shields.io/github/workflow/status/ellie/atuin/Rust?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/v/atuin.svg?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/d/atuin.svg?style=flat-square" /></a>
  <a href="https://github.com/ellie/atuin/blob/main/LICENSE"><img src="https://img.shields.io/crates/l/atuin.svg?style=flat-square" /></a>
</p>
 
A'tuin manages and synchronizes your shell history! Instead of storing
everything in a text file (such as ~/.history), A'tuin uses a sqlite database.
While being a little more complex, this allows for more functionality.

As well as the expected command, A'tuin stores

- duration
- exit code
- working directory
- hostname
- time
- a unique session ID

## Supported Shells

- zsh

## Requirements

- [fzf](https://github.com/junegunn/fzf)

## Install

### AUR

A'tuin is available on the [AUR](https://aur.archlinux.org/packages/atuin/)

```
yay -S atuin # or your AUR helper of choice
```

### With cargo

`atuin` needs a nightly version of Rust + Cargo! It's best to use
[rustup](https://rustup.rs/) for getting set up there.

```
rustup default nightly

cargo install atuin
```

### From source

```
rustup default nightly
git clone https://github.com/ellie/atuin.git
cd atuin
cargo install --path .
```

### Shell plugin

Once the binary is installed, the shell plugin requires installing. Add

```
eval "$(atuin init)"
```

to your `.zshrc`/`.bashrc`/whatever your shell uses.

## Usage

### History search

By default A'tuin will rebind ctrl-r to use fzf to fuzzy search your history.
It will also rebind the up arrow to use fzf, just without sorting. You can
prevent this by putting

```
export ATUIN_BINDKEYS="false"
```

into your shell config.

You may also change the default history selection. The default behaviour will search your entire history, however

```
export ATUIN_HISTORY="atuin history list --cwd"
```

will switch to only searching history for the current directory.

Similarly,

```
export ATUIN_HISTORY="atuin history list --session"
```

will search for the current session only, and

```
export ATUIN_HISTORY="atuin history list --session --cwd"
```

will do both!

### Import history

```
atuin import auto # detect shell, then import

or

atuin import zsh  # specify shell
```

### List history

List all history

```
atuin history list
```

List history for the current directory

```
atuin history list --cwd

atuin h l -c # alternative, shorter version
```

List history for the current session

```
atuin history list --session

atuin h l -s # similarly short
```

### Stats

A'tuin can calculate statistics for a single day, and accepts "natural language" style date input, as well as absolute dates:

```
$ atuin stats day last friday

+---------------------+------------+
| Statistic           | Value      |
+---------------------+------------+
| Most used command   | git status |
+---------------------+------------+
| Commands ran        |        450 |
+---------------------+------------+
| Unique commands ran |        213 |
+---------------------+------------+

$ atuin stats day 01/01/21 # also accepts absolute dates
```

It can also calculate statistics for all of known history:

```
$ atuin stats all

+---------------------+-------+
| Statistic           | Value |
+---------------------+-------+
| Most used command   |    ls |
+---------------------+-------+
| Commands ran        |  8190 |
+---------------------+-------+
| Unique commands ran |  2996 |
+---------------------+-------+
```

## Config

A'tuin is configurable via TOML. The file lives at ` ~/.config/atuin/config.toml`,
and looks like this:

```
[local]
dialect = "uk" # or us. sets the date format used by stats
server_address = "https://atuin.elliehuxtable.com/" # the server to sync with

[local.db]
path = "~/.local/share/atuin/history.db" # the local database for history
```

## ...what's with the name?

A'tuin is named after "The Great A'tuin", a giant turtle from Terry Pratchett's
Discworld series of books.
