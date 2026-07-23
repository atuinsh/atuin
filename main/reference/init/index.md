# init

## `atuin init <shell>`

Prints the shell plugin for the given shell. Evaluating its output is what installs Atuin's hooks and key bindings into your session, so this command belongs in your shell's startup file rather than being run by hand.

```
atuin init zsh
```

See [installation](https://docs.atuin.sh/guide/installation/#installing-the-shell-plugin) for the exact line to add for your shell — the syntax differs between shells.

Supported shells: `zsh`, `bash`, `fish`, `nu`, `xonsh`, `powershell`.

## What it sets up

- **Hooks** that record each command, its exit code, and its duration. See [Shell Integration](https://docs.atuin.sh/guide/shell-integration/index.md).
- **Key bindings** for `Ctrl`+`R` and the `Up` arrow, and `?` for [Atuin AI](https://docs.atuin.sh/ai/introduction/index.md).
- **Dotfiles**, if [enabled](https://docs.atuin.sh/configuration/config/#dotfiles) — your synced aliases and environment variables are defined here.

## Flags

| Flag                 | Description                                                                  |
| -------------------- | ---------------------------------------------------------------------------- |
| `--disable-up-arrow` | Don't bind the `Up` arrow key                                                |
| `--disable-ctrl-r`   | Don't bind `Ctrl`+`R`                                                        |
| `--disable-ai`       | Don't bind `?` to [Atuin AI](https://docs.atuin.sh/ai/introduction/index.md) |

For example, to keep `Ctrl`+`R` but leave the up arrow alone:

```
eval "$(atuin init zsh --disable-up-arrow)"
```

## Environment variables

| Variable                   | Effect                                                                                            |
| -------------------------- | ------------------------------------------------------------------------------------------------- |
| `ATUIN_NOBIND`             | If set to any value, binds no keys at all. Equivalent to passing every `--disable-*` flag.        |
| `ATUIN_NO_BUILTIN_PREEXEC` | Bash only. Stops `atuin init bash` from auto-loading its bundled bash-preexec (Atuin >= 18.18.0). |

Binding no keys is useful when you want to choose the bindings yourself:

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"

bindkey '^r' atuin-search
```

See [Key Binding](https://docs.atuin.sh/configuration/key-binding/index.md) for the widget and function names each shell exposes, and [Advanced Key Binding](https://docs.atuin.sh/configuration/advanced-key-binding/index.md) for customizing the keys *inside* the TUI.
