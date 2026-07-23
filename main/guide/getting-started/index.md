# Getting Started

This guide walks through setting Atuin up properly, one step at a time. If you just want the commands, the [quickstart on the home page](https://docs.atuin.sh/#quickstart) has them.

There are four steps, and only the first is required:

1. [Install Atuin](https://docs.atuin.sh/guide/installation/index.md) and its shell plugin
1. [Import your existing history](https://docs.atuin.sh/guide/import/index.md)
1. [Set up sync](https://docs.atuin.sh/guide/sync/index.md), if you want your history on more than one machine
1. [Learn the TUI](https://docs.atuin.sh/guide/basic-usage/index.md)

## 1. Install

The install script handles the binary and the shell plugin, and walks you through the rest:

```
bash <(curl --proto '=https' --tlsv1.2 -sSf https://setup.atuin.sh)
```

Then restart your shell. Prefer a package manager, or want to install the pieces yourself? See [Installation](https://docs.atuin.sh/guide/installation/index.md).

At this point Atuin is recording new commands. Press `Ctrl`+`R` or the `Up` arrow to search them.

## 2. Import your existing history

Atuin only records commands run after it's installed, so bring the old ones in:

```
atuin import auto
```

This detects your shell and imports from its history file, which stays untouched and keeps being written to. See [Import existing history](https://docs.atuin.sh/guide/import/index.md) for importing from a specific shell or a non-default file.

## 3. Set up sync (optional)

Sync backs your history up and shares it between machines, end-to-end encrypted. You can use our server or [host your own](https://docs.atuin.sh/self-hosting/server-setup/index.md).

```
atuin register -u <USERNAME> -e <EMAIL>
atuin sync
```

Registration generates an encryption key. **Save it somewhere safe** — you'll need it to log in on any other machine, and it cannot be recovered. See [Setting up sync](https://docs.atuin.sh/guide/sync/index.md) for the details, including logging in elsewhere.

Skipping this step is fine. Your history stays on this machine, not backed up and not synchronized.

## 4. Make it yours

Once you're up and running:

- [Basic usage](https://docs.atuin.sh/guide/basic-usage/index.md) — what Atuin records, and how to drive the TUI
- [Filter and search modes](https://docs.atuin.sh/guide/advanced-usage/index.md) — narrow searches to this directory, this machine, or this session
- [Key bindings](https://docs.atuin.sh/configuration/key-binding/index.md) — including how to [disable the up arrow binding](https://docs.atuin.sh/configuration/key-binding/#disable-up-arrow) if it isn't for you
- [Config](https://docs.atuin.sh/configuration/config/index.md) — every setting, including an [inline window](https://docs.atuin.sh/configuration/config/#inline_height) instead of full screen

## Getting help

Open a topic on the [forum](https://forum.atuin.sh), join our [Discord](https://discord.gg/Fq8bJSKPHh), or file an [issue](https://github.com/atuinsh/atuin/issues). Running [`atuin doctor`](https://docs.atuin.sh/reference/doctor/index.md) collects the information we'll ask for.
