# Atuin

Atuin replaces your shell history with a SQLite database, and records extra context for every command: the directory it ran in, how long it took, whether it succeeded, and which machine and session it came from. That context is what makes search actually useful.

It can also sync your history across all of your machines, end-to-end encrypted. Use our server, [host your own](https://docs.atuin.sh/self-hosting/server-setup/index.md), or skip sync entirely and stay local.

## Quickstart

```
bash <(curl --proto '=https' --tlsv1.2 -sSf https://setup.atuin.sh)
```

Restart your shell, then press `Ctrl`+`R` or the `Up` arrow to search. Type a query, press enter to run the selected command, or tab to put it on your command line for editing.

To bring your existing history with you:

```
atuin import auto
```

To sync it across machines — optional, and covered in [Setting up sync](https://docs.atuin.sh/guide/sync/index.md):

```
atuin register -u <USERNAME> -e <EMAIL>
atuin sync
```

- **Get set up**

  ______________________________________________________________________

  [Installation](https://docs.atuin.sh/guide/installation/index.md) · [Import history](https://docs.atuin.sh/guide/import/index.md) · [Set up sync](https://docs.atuin.sh/guide/sync/index.md)

- **Use it well**

  ______________________________________________________________________

  [Basic usage](https://docs.atuin.sh/guide/basic-usage/index.md) · [Filter and search modes](https://docs.atuin.sh/guide/advanced-usage/index.md) · [Key bindings](https://docs.atuin.sh/configuration/key-binding/index.md)

- **Tune it**

  ______________________________________________________________________

  [All config options](https://docs.atuin.sh/configuration/config/index.md) · [Theming](https://docs.atuin.sh/guide/theming/index.md) · [Excluding commands](https://docs.atuin.sh/guide/excluding-commands/index.md)

- **Go further**

  ______________________________________________________________________

  [Atuin AI](https://docs.atuin.sh/ai/introduction/index.md) · [AI agent hooks](https://docs.atuin.sh/guide/agent-hooks/index.md) · [Self-hosting](https://docs.atuin.sh/self-hosting/server-setup/index.md)

## Supported shells

zsh, bash, fish, nushell, xonsh, and PowerShell (tier 2 support).

## Getting help

Open a topic on the [forum](https://forum.atuin.sh), join our [Discord](https://discord.gg/Fq8bJSKPHh), or file an [issue](https://github.com/atuinsh/atuin/issues). If something isn't working, [`atuin doctor`](https://docs.atuin.sh/reference/doctor/index.md) collects the details we'll ask for.
