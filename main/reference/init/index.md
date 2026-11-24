# init

## `atuin init <shell>`

Prints the shell plugin for the given shell. Evaluating its output is what installs Atuin's hooks and key bindings into your session, so this command belongs in your shell's startup file rather than being run by hand.

```
atuin init zsh
```

`atuin setup` adds the `atuin init` line to the startup files of zsh, bash, fish and PowerShell. To add it without the setup screen, pass `--only` and name the shells with `--shells`:

```
atuin setup --only --shell-history --shells zsh,bash,fish,powershell
```

| Shell      | Startup file that setup writes to                                                                                                |
| ---------- | -------------------------------------------------------------------------------------------------------------------------------- |
| zsh        | `$ZDOTDIR/.zshrc`, or `~/.zshrc` when `ZDOTDIR` isn't set                                                                        |
| bash       | `~/.bashrc`                                                                                                                      |
| fish       | `~/.config/fish/config.fish`, inside an `if status is-interactive` block                                                         |
| PowerShell | `~/.config/powershell/Microsoft.PowerShell_profile.ps1`, or on Windows, the profiles of both PowerShell 7 and Windows PowerShell |

Setup leaves a startup file that already runs `atuin init` as it is. `atuin setup --only --no-shell-history` takes out the lines that setup wrote.

For Nushell and xonsh, add the line to your startup file yourself. See [installation](https://docs.atuin.sh/guide/installation/#installing-the-shell-plugin) for the exact line to add for your shell — the syntax differs between shells.

Supported shells: `zsh`, `bash`, `fish`, `nu`, `xonsh`, `powershell`. See [Supported platforms](https://docs.atuin.sh/support/index.md) for what each tier means.

## What it sets up

- **Hooks** that record each command, its exit code, and its duration. See [Shell Integration](https://docs.atuin.sh/guide/shell-integration/index.md).
- **Key bindings** for `Ctrl`+`R` and the `Up` arrow, which open interactive search. If [`threads.enabled`](https://docs.atuin.sh/ai/threads/index.md) is true, `Ctrl`+`T` is also bound to `atuin resume`; see [Thread resume key](https://docs.atuin.sh/configuration/key-binding/#thread-resume-key).

## Environment variables

| Variable                   | Effect                                                                                                     |
| -------------------------- | ---------------------------------------------------------------------------------------------------------- |
| `ATUIN_NOBIND`             | If set to any value, binds no keys at all.                                                                 |
| `ATUIN_NO_BUILTIN_PREEXEC` | Bash only. Stops `atuin init bash` from automatically loading its bundled bash-preexec (Atuin >= 18.18.0). |

Binding no keys is useful when you want to choose the bindings yourself:

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"

bindkey '^r' atuin-search
```

See [Key Binding](https://docs.atuin.sh/configuration/key-binding/index.md) for the widget and function names each shell exposes, and [Advanced Key Binding](https://docs.atuin.sh/configuration/advanced-key-binding/index.md) for customizing the keys *inside* the TUI.
