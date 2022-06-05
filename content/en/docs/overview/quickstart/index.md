---
title: "Quickstart"
description: ""
lead: ""
date: 2022-06-05T21:27:41+01:00
lastmod: 2022-06-05T21:27:41+01:00
draft: false
images: []
weight: 2
menu:
  docs:
    parent: "overview"
type: docs
---

## With the default sync server
  
This will sign you up for the default sync server, hosted by me. Everything is end-to-end encrypted, so your secrets are safe!
  
Read more below for offline-only usage, or for hosting your own server.

```
bash <(curl https://raw.githubusercontent.com/ellie/atuin/main/install.sh)

atuin register -u <USERNAME> -e <EMAIL> -p <PASSWORD>
atuin import auto
atuin sync
```

### Opt-in to activity graph
Alongside the hosted Atuin server, there is also a service which generates activity graphs for your shell history! These are inspired by the GitHub graph.
  
For example, here is mine:
  
![](https://api.atuin.sh/img/ellie.png?token=0722830c382b42777bdb652da5b71efb61d8d387)
  
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

## Install

### Script (recommended)

The install script will help you through the setup, ensuring your shell is
properly configured. It will also use one of the below methods, preferring the
system package manager where possible (pacman, homebrew, etc etc).

```
# do not run this as root, root will be asked for if required
bash <(curl https://raw.githubusercontent.com/ellie/atuin/main/install.sh)
```

### Cargo

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

### Pacman

Atuin is available in the Arch Linux [community repository](https://archlinux.org/packages/community/x86_64/atuin/):

```
pacman -S atuin
```
  
And then follow [the shell setup](#shell-plugin)

### From source

```
git clone https://github.com/ellie/atuin.git
cd atuin
cargo install --path .
```
  
And then follow [the shell setup](#shell-plugin)

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
