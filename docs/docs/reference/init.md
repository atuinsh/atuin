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

Supported shells: `zsh`, `bash`, `fish`, `nu`, `xonsh`, `powershell`. See
[Supported platforms](../support.md) for what each tier means.

## What it sets up

- **Hooks** that record each command, its exit code, and its duration. See
  [Shell Integration](../guide/shell-integration.md).
- **Key bindings** for ++ctrl+r++ and the ++up++ arrow, ++question++ for
  [Atuin AI](../ai/introduction.md), and ++ctrl+bracket-right++ for `atuin ai resume`
  (zsh, bash and fish).

## Flags

| Flag | Description |
|------|-------------|
| `--disable-up-arrow` | Don't bind the ++up++ arrow key |
| `--disable-ctrl-r` | Don't bind ++ctrl+r++ |
| `--disable-ai` | Don't bind ++question++ to [Atuin AI](../ai/introduction.md) |
| `--disable-ai-resume` | Don't bind ++ctrl+bracket-right++ to `atuin ai resume` |

!!! note "++ctrl+bracket-right++ replaces a shell binding"
    By default ++ctrl+bracket-right++ is zsh's `vi-find-next-char` (emacs keymap) and
    bash's `character-search`: jump to the next occurrence of the character you type next.
    `atuin init` rebinds it in the emacs, vi-insert and vi-command keymaps. To keep the
    shell's binding, pass `--disable-ai-resume`, or set `ATUIN_NOBIND` and bind the resume
    widget to a key of your choice, for example in zsh:

    ```shell
    eval "$(atuin init zsh --disable-ai-resume)"
    bindkey '^x^r' atuin-ai-resume
    ```

    In bash, use `atuin-bind '\C-x\C-r' atuin-ai-resume` (see
    [Key Binding](../configuration/key-binding.md)).

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
