# Getting Started

This guide walks through setting Atuin up properly, one step at a time. If you
just want the commands, the [quickstart on the home page](../index.md#quickstart)
has them.

Four steps follow, and only the first is required:

1. [Install Atuin](installation.md) and its shell plugin
2. [Import your existing history](import.md)
3. [Set up sync](sync.md), if you want your history on more than one machine
4. [Learn the TUI](basic-usage.md)

## 1. Install

The install script handles the binary and the shell plugin, and walks you
through the rest:

```shell
bash <(curl --proto '=https' --tlsv1.2 -sSf https://setup.atuin.sh)
```

Then restart your shell. Prefer a package manager, or want to install the pieces
yourself? See [Installation](installation.md).

At this point Atuin is recording new commands. Press ++ctrl+r++ or the ++up++
arrow to search them.

## 2. Import your existing history

Atuin only records commands run after it's installed, so bring the old ones in:

```shell
atuin import auto
```

This detects your shell and imports from its history file, which stays untouched
and keeps being written to. See [Import existing history](import.md) for
importing from a specific shell or a non-default file.

## 3. Set up sync (optional)

Sync backs your history up and shares it between machines, end-to-end encrypted.
You can use our server or [host your own](../self-hosting/server-setup.md).

```shell
atuin register -u <USERNAME> -e <EMAIL>
atuin sync
```

Registration generates an encryption key. **Save it somewhere safe**—you'll
need it to log in on any other machine, and it can't be recovered. See
[Setting up sync](sync.md) for the details, including logging in elsewhere.

Skipping this step is fine. Your history stays on this machine, not backed up
and not synchronized.

## 4. Make it yours

Once you're up and running:

- [Basic usage](basic-usage.md)—what Atuin records, and how to drive the TUI
- [Filter and search modes](advanced-usage.md)—narrow searches to this
  directory, this machine, or this session
- [Key bindings](../configuration/key-binding.md)—including how to
  [disable the up arrow binding](../configuration/key-binding.md#disable-up-arrow)
  if it isn't for you
- [Config](../configuration/config.md)—every setting, including an
  [inline window](../configuration/config.md#inline_height) instead of full screen

## Getting help

Open a topic on the [forum](https://forum.atuin.sh), join our
[Discord](https://discord.gg/Fq8bJSKPHh), or file an
[issue](https://github.com/atuinsh/atuin/issues). Running
[`atuin doctor`](../reference/doctor.md) collects the information we'll ask for.
