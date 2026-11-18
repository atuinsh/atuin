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
- **Key bindings** for `Ctrl`+`R` and the `Up` arrow.
- **Widgets** for `atuin resume` (zsh, bash and fish), unbound unless you ask for them: see [Binding `atuin resume`](#binding-atuin-resume).

## Flags

| Flag                 | Description                                                            |
| -------------------- | ---------------------------------------------------------------------- |
| `--disable-up-arrow` | Don't bind the `Up` arrow key                                          |
| `--disable-ctrl-r`   | Don't bind `Ctrl`+`R`                                                  |
| `--bind-ai-resume`   | Bind `Ctrl`+`]` to `atuin resume` (zsh, bash and fish; off by default) |

For example, to keep `Ctrl`+`R` but leave the up arrow alone:

```
eval "$(atuin init zsh --disable-up-arrow)"
```

`atuin setup --only --no-up-arrow` and `atuin setup --only --no-ctrl-r` turn the same keys off through the [`bind_up_arrow` and `bind_ctrl_r`](https://docs.atuin.sh/configuration/config/#bind_ctrl_r-and-bind_up_arrow) settings, and leave the `atuin init` line as it is.

## Binding `atuin resume`

`atuin init` defines widgets that open the [`atuin resume`](https://docs.atuin.sh/ai/threads/index.md) picker at the prompt and put the command it picks into your shell, but binds no key to them by default. To bind `Ctrl`+`]` in the emacs, vi-insert and vi-command keymaps, pass `--bind-ai-resume`:

```
eval "$(atuin init zsh --bind-ai-resume)"
```

`Ctrl`+`]` replaces a shell binding

`Ctrl`+`]` is zsh's `vi-find-next-char` (emacs keymap) and bash's `character-search`: jump to the next occurrence of the character you type next. `--bind-ai-resume` replaces it. To keep it, bind the widget to another key yourself.

To bind a key of your choice instead, add it after the `atuin init` line:

```
eval "$(atuin init zsh)"
bindkey -M emacs '^x^r' atuin-ai-resume
bindkey -M viins '^x^r' atuin-ai-resume-viins
bindkey -M vicmd '^x^r' atuin-ai-resume-vicmd
```

```
eval "$(atuin init bash)"
atuin-bind -m emacs      '\C-x\C-r' atuin-ai-resume-emacs
atuin-bind -m vi-insert  '\C-x\C-r' atuin-ai-resume-viins
atuin-bind -m vi-command '\C-x\C-r' atuin-ai-resume-vicmd
```

`atuin-bind` is the helper `atuin init bash` defines (see [Key Binding](https://docs.atuin.sh/configuration/key-binding/index.md)); it also works under ble.sh.

```
atuin init fish | source
bind ctrl-x,ctrl-r _atuin_ai_resume
bind -M insert ctrl-x,ctrl-r _atuin_ai_resume
```

(Fish 3.x spells the key `\cx\cr`.)

## Environment variables

| Variable                   | Effect                                                                                                     |
| -------------------------- | ---------------------------------------------------------------------------------------------------------- |
| `ATUIN_NOBIND`             | If set to any value, binds no keys at all. Equivalent to passing every `--disable-*` flag.                 |
| `ATUIN_NO_BUILTIN_PREEXEC` | Bash only. Stops `atuin init bash` from automatically loading its bundled bash-preexec (Atuin >= 18.18.0). |

Binding no keys is useful when you want to choose the bindings yourself:

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"

bindkey '^r' atuin-search
```

See [Key Binding](https://docs.atuin.sh/configuration/key-binding/index.md) for the widget and function names each shell exposes, and [Advanced Key Binding](https://docs.atuin.sh/configuration/advanced-key-binding/index.md) for customizing the keys *inside* the TUI.
