---
title: Advanced Install
---
Generally, we recommend using our install script. It ensures you use the most
up-to-date Atuin, and that your shell plugin is correctly setup. It will prefer
the system package manager wherever necessary!

However, I totally understand if you'd rather do things yourself and not run a
script from the internet. If so, follow on!

## Install Atuin

Atuin is in a number of package repositories! Please choose whichever works best for you.

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
nix profile install "github:atuinsh/atuin"
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

Note: Atuin builds on the latest stable version of Rust, and we make no
promises regarding older versions. We recommend using rustup.

```
git clone https://github.com/atuinsh/atuin.git
cd atuin/atuin
cargo install --path .
```

## Shell plugin

Once the binary is installed, the shell plugin requires installing. If you use
the install script, this should all be done for you! After installing, remember to restart your shell.

### zsh

```
echo 'eval "$(atuin init zsh)"' >> ~/.zshrc
```

#### Zinit

```sh
zinit load atuinsh/atuin
```

#### Antigen  
  
```sh  
antigen bundle atuinsh/atuin@main
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
