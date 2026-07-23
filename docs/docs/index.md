# Atuin

Atuin replaces your shell history with a SQLite database, and records extra
context for every command: the directory it ran in, how long it took, whether it
succeeded, and which machine and session it came from. That context is what
makes search actually useful.

It can also sync your history across all of your machines, end-to-end encrypted.
Use our server, [host your own](self-hosting/server-setup.md), or skip sync
entirely and stay local.

## Quickstart

```bash
bash <(curl --proto '=https' --tlsv1.2 -sSf https://setup.atuin.sh)
```

Restart your shell, then press ++ctrl+r++ or the ++up++ arrow to search. Type a
query, press enter to run the selected command, or tab to put it on your command
line for editing.

To bring your existing history with you:

```bash
atuin import auto
```

To sync it across machines — optional, and covered in
[Setting up sync](guide/sync.md):

```bash
atuin register -u <USERNAME> -e <EMAIL>
atuin sync
```

<div class="grid cards" markdown>

-   **Get set up**

    ---

    [Installation](guide/installation.md) ·
    [Import history](guide/import.md) ·
    [Set up sync](guide/sync.md)

-   **Use it well**

    ---

    [Basic usage](guide/basic-usage.md) ·
    [Filter and search modes](guide/advanced-usage.md) ·
    [Key bindings](configuration/key-binding.md)

-   **Tune it**

    ---

    [All config options](configuration/config.md) ·
    [Theming](guide/theming.md) ·
    [Excluding commands](guide/excluding-commands.md)

-   **Go further**

    ---

    [Atuin AI](ai/introduction.md) ·
    [AI agent hooks](guide/agent-hooks.md) ·
    [Self-hosting](self-hosting/server-setup.md)

</div>

## Supported shells

zsh, bash, fish, nushell, xonsh, and PowerShell (tier 2 support).

## Getting help

Open a topic on the [forum](https://forum.atuin.sh), join our
[Discord](https://discord.gg/Fq8bJSKPHh), or file an
[issue](https://github.com/atuinsh/atuin/issues). If something isn't working,
[`atuin doctor`](reference/doctor.md) collects the details we'll ask for.
