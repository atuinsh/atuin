# Getting started

Atuin replaces your existing shell history with a SQLite database, and records
additional context for your commands. With this context, Atuin gives you faster
and better search of your shell history.

Additionally, Atuin (optionally) syncs your shell history between all of your
machines. Fully end-to-end encrypted, of course.

You may use either the server I host, or host your own! Or just don't use sync
at all. As all history sync is encrypted, I couldn't access your data even if I
wanted to. And I **really** don't want to.

If you have any problems, please open a topic on the [forum](https://forum.atuin.sh)

Alternatively, get in touch on our [Discord](https://discord.gg/Fq8bJSKPHh) or open an [issue](https://github.com/atuinsh/atuin/issues)

#### Supported Shells

- zsh
- bash
- fish
- nushell
- xonsh
- powershell (tier 2 support)

## Quickstart

Please do try and read this guide, but if you're in a hurry and want to get
started quickly:

```bash
bash <(curl --proto '=https' --tlsv1.2 -sSf https://setup.atuin.sh)

atuin register -u <USERNAME> -e <EMAIL>
atuin import auto
atuin sync
```

Now restart your shell!

Anytime you press ctrl-r or up, you will see the Atuin search UI. Type your
query, enter to execute. If you'd like to select a command without executing
it, press tab.

You might like to configure an [inline window](configuration/config.md#inline_height), or [disable up arrow bindings](configuration/key-binding.md#disable-up-arrow)

[**Installation** - Install and setup Atuin](guide/installation.md)
