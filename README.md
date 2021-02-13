<h1 align="center">
  A'tuin
</h1>
<blockquote align="center">
  Through the fathomless deeps of space swims the star turtle Great A’Tuin, bearing on its back the four giant elephants who carry on their shoulders the mass of the Discworld.
 </blockquote>
 
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

## Install

`atuin` needs a recent version of Rust + Cargo! It's best to use
[rustup](https://rustup.rs/) for getting set up there.

```
cargo install atuin
```

Once the binary is installed, the shell plugin requires installing:

zplug:

```
zplug "ellie/atuin"
```

antigen:

```
antigen use https://github.com/ellie/atuin.git
```

oh-my-zsh:

```
git clone https://github.com/ellie/atuin ~/.oh-my-zsh/plugins/atuin
```

and then add `atuin` to your `plugins` list in `~/.zshrc`

## Usage

By default A'tuin will rebind ctrl-r to use fzf to fuzzy search your history. You
can specify a different fuzzy tool by changing the value of `ATUIN_FUZZY`:

```
export ATUIN_FUZZY=fzy
```

### Import history

```
atuin import auto # detect shell, then import

or

atuin import zsh  # specify shell
```

### List history

```
atuin history list
```

## ...what's with the name?

A'tuin is named after "The Great A'tuin", a giant turtle from Terry Pratchett's
Discworld series of books.
