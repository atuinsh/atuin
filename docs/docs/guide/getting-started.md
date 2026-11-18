# Getting Started

This guide walks through setting Atuin up properly, one step at a time. If you
just want the commands, the [quickstart on the home page](../index.md#quickstart)
has them.

Setting Atuin up takes four steps, and only the first is required. The install
script runs `atuin setup`, which walks you through the first three:

1. [Install Atuin](installation.md) and its shell plugin
2. [Import your existing history](import.md)
3. [Set up sync](sync.md), if you want your history on more than one machine
4. [Learn the TUI](basic-usage.md)

## 1. Install

The install script installs the binary, then runs `atuin setup`, which installs
the shell plugin and walks you through the rest:

```shell
curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh
```

Then restart your shell. Prefer a package manager, or want to install the pieces
yourself? See [Installation](installation.md). After installing the binary another
way, run `atuin setup` to do the rest.

At this point Atuin is recording new commands. Press ++ctrl+r++ or the ++up++
arrow to search them.

## 2. Import your existing history

Atuin only records commands run after it's installed, so setup brings the old
ones in. Setup imports your current shell's history file, which stays untouched —
your shell keeps writing to it as usual.

If you skipped the import, or your sync account already had history, import this
machine's history with:

```shell
atuin import auto
```

See [Import existing history](import.md) for importing from a specific shell or a
non-default file.

## 3. Set up sync (optional)

Sync backs your history up and shares it between machines, end-to-end encrypted.
You can use our server or [host your own](../self-hosting/server-setup.md).

Setup offers to sign you in to Atuin Cloud. If you skipped sync, sign in through
your browser with:

```shell
atuin setup --sync cloud
```

Atuin encrypts your history with a key that it keeps on this machine. Run
`atuin key` to show the key. **Save the key somewhere safe** — you'll need it to
log in on any other machine, and it can't be recovered. See
[Setting up sync](sync.md) for the details, including logging in elsewhere.

Skipping this step is fine. Your history stays on this machine, not backed up
and not synchronized.

## 4. Make it yours

Once you're up and running:

- [Basic usage](basic-usage.md) — what Atuin records, and how to drive the TUI
- [Filter and search modes](advanced-usage.md) — narrow searches to this
  directory, this machine, or this session
- [Key bindings](../configuration/key-binding.md) — including how to
  [disable the up arrow binding](../configuration/key-binding.md#disable-up-arrow)
  with `atuin setup --only --no-up-arrow` if it isn't for you
- [Config](../configuration/config.md) — every setting, including an
  [inline window](../configuration/config.md#inline_height) instead of full screen

## Getting help

Open a topic on the [forum](https://forum.atuin.sh), join our
[Discord](https://discord.gg/Fq8bJSKPHh), or file an
[issue](https://github.com/atuinsh/atuin/issues). Running
[`atuin doctor`](../reference/doctor.md) collects the information we'll ask for.
