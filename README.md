<h1 align="center">
  A'Tuin
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
 
A'Tuin manages and synchronizes your shell history! Instead of storing
everything in a text file (such as ~/.history), A'Tuin uses a sqlite database.
While being a little more complex, this allows for more functionality.

As well as the expected command, A'Tuin stores

- duration
- exit code
- working directory
- hostname
- time
- a unique session ID

## Supported Shells

- zsh

## Install

### AUR

A'Tuin is available on the [AUR](https://aur.archlinux.org/packages/atuin/)

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

By default A'Tuin will rebind ctrl-r and the up arrow to search your history.

You can prevent this by putting

```
export ATUIN_BINDKEYS="false"
```

into your shell config.

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

A'Tuin can calculate statistics for a single day, and accepts "natural language" style date input, as well as absolute dates:

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

A'Tuin is configurable via TOML. The file lives at ` ~/.config/atuin/config.toml`,
and looks like this:

```
[local]
dialect = "uk" # or us. sets the date format used by stats
server_address = "https://atuin.elliehuxtable.com/" # the server to sync with

[local.db]
path = "~/.local/share/atuin/history.db" # the local database for history
```

## ...what's with the name?

A'Tuin is named after "The Great A'Tuin", a giant turtle from Terry Pratchett's
Discworld series of books.
