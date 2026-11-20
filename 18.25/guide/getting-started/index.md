# Getting Started

This guide walks through setting Atuin up properly, one step at a time. If you just want the commands, the [quickstart on the home page](https://docs.atuin.sh/#quickstart) has them.

Setting Atuin up takes four steps, and only the first is required. The install script runs `atuin setup`, which walks you through the first three:

1. [Install Atuin](https://docs.atuin.sh/guide/installation/index.md) and its shell plugin
1. [Import your existing history](https://docs.atuin.sh/guide/import/index.md)
1. [Set up sync](https://docs.atuin.sh/guide/sync/index.md), if you want your history on more than one machine
1. [Learn the TUI](https://docs.atuin.sh/guide/basic-usage/index.md)

## 1. Install

The install script installs the binary, then runs `atuin setup`, which installs the shell plugin and walks you through the rest:

```
curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh
```

Then restart your shell. Prefer a package manager, or want to install the pieces yourself? See [Installation](https://docs.atuin.sh/guide/installation/index.md). After installing the binary another way, run `atuin setup` to do the rest.

At this point Atuin is recording new commands. Press `Ctrl`+`R` or the `Up` arrow to search them.

## 2. Import your existing history

Atuin only records commands run after it's installed, so setup brings the old ones in. Setup imports your current shell's history file, which stays untouched — your shell keeps writing to it as usual.

If you skipped the import, or your sync account already had history, import this machine's history with:

```
atuin import auto
```

See [Import existing history](https://docs.atuin.sh/guide/import/index.md) for importing from a specific shell or a non-default file.

## 3. Set up sync (optional)

Sync backs your history up and shares it between machines, end-to-end encrypted. You can use our server or [host your own](https://docs.atuin.sh/self-hosting/server-setup/index.md).

Setup offers to sign you in to Atuin Cloud. If you skipped sync, sign in through your browser with:

```
atuin setup --sync cloud
```

Atuin encrypts your history with a key that it keeps on this machine. Run `atuin key` to show the key. **Save the key somewhere safe** — you'll need it to log in on any other machine, and it can't be recovered. See [Setting up sync](https://docs.atuin.sh/guide/sync/index.md) for the details, including logging in elsewhere.

Skipping this step is fine. Your history stays on this machine, not backed up and not synchronized.

## 4. Make it yours

Once you're up and running:

- [Basic usage](https://docs.atuin.sh/guide/basic-usage/index.md) — what Atuin records, and how to drive the TUI
- [Filter and search modes](https://docs.atuin.sh/guide/advanced-usage/index.md) — narrow searches to this directory, this machine, or this session
- [Key bindings](https://docs.atuin.sh/configuration/key-binding/index.md) — including how to [disable the up arrow binding](https://docs.atuin.sh/configuration/key-binding/#disable-up-arrow) with `atuin setup --only --no-up-arrow` if it isn't for you
- [Config](https://docs.atuin.sh/configuration/config/index.md) — every setting, including an [inline window](https://docs.atuin.sh/configuration/config/#inline_height) instead of full screen

## Getting help

Open a topic on the [forum](https://forum.atuin.sh), join our [Discord](https://discord.gg/Fq8bJSKPHh), or file an [issue](https://github.com/atuinsh/atuin/issues). Running [`atuin doctor`](https://docs.atuin.sh/reference/doctor/index.md) collects the information we'll ask for.
