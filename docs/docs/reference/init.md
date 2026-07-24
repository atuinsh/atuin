# init

## `atuin init <shell>`

Prints the shell plugin for the given shell. Evaluating its output is what
installs Atuin's hooks and key bindings into your session, so this command
belongs in your shell's startup file rather than being run by hand.

```shell
atuin init zsh
```

See [installation](../guide/installation.md#installing-the-shell-plugin) for the
exact line to add for your shell — the syntax differs between shells.

Supported shells: `zsh`, `bash`, `fish`, `nu`, `xonsh`, `powershell`.

## What it sets up

- **Hooks** that record each command, its exit code, and its duration. See
  [Shell Integration](../guide/shell-integration.md).
- **Key bindings** for ++ctrl+r++ and the ++up++ arrow, and ++question++ for
  [Atuin AI](../ai/introduction.md).
- **Dotfiles**, if [enabled](../configuration/config.md#dotfiles) — your synced
  aliases and environment variables are defined here.

## Flags

| Flag | Description |
|------|-------------|
| `--disable-up-arrow` | Don't bind the ++up++ arrow key |
| `--disable-ctrl-r` | Don't bind ++ctrl+r++ |
| `--disable-ai` | Don't bind ++question++ to [Atuin AI](../ai/introduction.md) |

For example, to keep ++ctrl+r++ but leave the up arrow alone:

```shell
eval "$(atuin init zsh --disable-up-arrow)"
```

## Environment variables

| Variable | Effect |
|----------|--------|
| `ATUIN_NOBIND` | If set to any value, binds no keys at all. Equivalent to passing every `--disable-*` flag. |
| `ATUIN_NO_BUILTIN_PREEXEC` | Bash only. Stops `atuin init bash` from automatically loading its bundled bash-preexec (Atuin >= 18.18.0). |

Binding no keys is useful when you want to choose the bindings yourself:

```shell
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"

bindkey '^r' atuin-search
```

See [Key Binding](../configuration/key-binding.md) for the widget and function
names each shell exposes, and
[Advanced Key Binding](../configuration/advanced-key-binding.md) for customizing
the keys *inside* the TUI.
