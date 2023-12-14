---
title: Getting Started
id: index
slug: /
sidebar_position: 1
---

Atuin replaces your existing shell history with a SQLite database, and records
additional context for your commands. With this context, Atuin gives you faster
and better search of your shell history!

Additionally, Atuin (optionally) syncs your shell history between all of your
machines! Fully end-to-end encrypted, of course.

You may use either the server I host, or host your own! Or just don't use sync
at all. As all history sync is encrypted, I couldn't access your data even if I
wanted to. And I **really** don't want to.

If you have any problems, please open an [issue](https://github.com/ellie/atuin/issues) or get in touch on our [Discord](https://discord.gg/Fq8bJSKPHh)!

## Supported Shells

- zsh
- bash
- fish
- nushell
 
# Quickstart

Please do try and read this guide, but if you're in a hurry and want to get
started quickly:

```
bash <(curl --proto '=https' --tlsv1.2 -sSf https://setup.atuin.sh)

atuin register -u <USERNAME> -e <EMAIL>
atuin import auto
atuin sync
```

Now restart your shell! 

Anytime you press ctrl-r or up, you will see the Atuin search UI. Type your
query, enter to execute. If you'd like to select a command without executing
it, press tab.

You might like to configure an [inline
window](https://atuin.sh/docs/config/#inline_height), or [disable up arrow
bindings](https://atuin.sh/docs/key-binding#disable-up-arrow)

# Full Guide

Let's get started! First up, you will want to install Atuin. We have an install
script which handles most of the commonly used platforms and package managers:

## bash/zsh

```
bash <(curl https://raw.githubusercontent.com/ellie/atuin/main/install.sh)
```

## fish

```
bash (curl --proto '=https' --tlsv1.2 -sSf https://setup.atuin.sh | psub)
```

## Importing

The script will install the binary and attempt to configure your shell. Atuin
uses a shell plugin to ensure that we capture new shell history. But for older
history, you will need to import it

This will import the history for your current shell:
```
atuin import auto
```

Alternatively, you can specify the shell like so:

```
atuin import bash
atuin import zsh # etc
```

## Register

At this point, you have Atuin storing and searching your shell history! But it
isn't syncing it just yet. To do so, you'll need to register with the sync
server. All of your history is fully end-to-end encrypted, so there are no
risks of the server snooping on you.

Note: if you already have an account and wish to sync with an additional
machine, follow the [below guide](#syncing-additional-machines) instead.

```
atuin register -u <YOUR_USERNAME> -e <YOUR EMAIL>
```

After registration, Atuin will generate an encryption key for you and store it
locally. This is needed for logging in to other machines, and can be seen with

```
atuin key
```

Please **never** share this key with anyone! The Atuin developers will never
ask you for your key, your password, or the contents of your Atuin directory.

If you lose your key, we can do nothing to help you. We recommend you store
this somewhere safe, such as in a password manager.

## First sync
By default, Atuin will sync your history once per hour. This can be
[configured](/docs/config#sync_frequency).

To run a sync manually, please run

```
atuin sync
```

Atuin tries to be smart with a sync, and not waste data transfer. However, if
you are seeing some missing data, please try running

```
atuin sync -f
```

This triggers a full sync, which may take longer as it works through historical data.

## Syncing additional machines

When only signed in on one machine, Atuin sync operates as a backup. This is
pretty useful by itself, but syncing multiple machines is where the magic
happens.

First, ensure you are [registered with the sync server](#register) and make a
note of your key. You can see this with `atuin key`.

Then, install Atuin on a new machine. Once installed, login with

```
atuin login -u <USERNAME>
```

You will be prompted for your password, and for your key.

Syncing will happen automatically in the background, but you may wish to run it manually with

```
atuin sync
```

Or, if you see missing data, force a full sync with: 

```
atuin sync -f
```
  
## Opt-in to activity graph
Alongside the hosted Atuin server, there is also a service which generates
activity graphs for your shell history! These are inspired by the GitHub graph.
  
For example, here is mine:
  
![Activity Graph Example](https://api.atuin.sh/img/ellie.png?token=0722830c382b42777bdb652da5b71efb61d8d387)

If you wish to get your own, after signing up for the sync server, run this
  
```
curl https://api.atuin.sh/enable -d $(cat ~/.local/share/atuin/session)
```
  
The response includes the URL to your graph. Feel free to share and/or embed
this URL, the token is _not_ a secret, and simply prevents user enumeration. 

## Known issues
- SQLite has some issues with ZFS in certain configurations. As Atuin uses SQLite, this may cause your shell to become slow! We have an [issue](https://github.com/atuinsh/atuin/issues/952) to track, with some workarounds
- SQLite also does not tend to like network filesystems (eg, NFS)
