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
- **Key bindings** for ++ctrl+r++ and the ++up++ arrow, and ++question++ for
  [Atuin AI](../ai/introduction.md).
- **Widgets** for `atuin resume` (zsh, bash and fish), unbound unless you ask
  for them: see [Binding `atuin resume`](#binding-atuin-resume).

## Flags

| Flag | Description |
|------|-------------|
| `--disable-up-arrow` | Don't bind the ++up++ arrow key |
| `--disable-ctrl-r` | Don't bind ++ctrl+r++ |
| `--disable-ai` | Don't bind ++question++ to [Atuin AI](../ai/introduction.md) |
| `--bind-ai-resume` | Bind ++ctrl+bracket-right++ to `atuin resume` (zsh, bash and fish; off by default) |

For example, to keep ++ctrl+r++ but leave the up arrow alone:

```shell
eval "$(atuin init zsh --disable-up-arrow)"
```

## Binding `atuin resume`

`atuin init` defines widgets that open the [`atuin resume`](../ai/sessions.md)
picker at the prompt and put the command it picks into your shell, but binds no
key to them by default. To bind ++ctrl+bracket-right++ in the emacs, vi-insert and
vi-command keymaps, pass `--bind-ai-resume`:

```shell
eval "$(atuin init zsh --bind-ai-resume)"
```

!!! note "++ctrl+bracket-right++ replaces a shell binding"
    ++ctrl+bracket-right++ is zsh's `vi-find-next-char` (emacs keymap) and bash's
    `character-search`: jump to the next occurrence of the character you type next.
    `--bind-ai-resume` replaces it. To keep it, bind the widget to another key yourself.

To bind a key of your choice instead, add it after the `atuin init` line:

=== "zsh"

    ```shell
    eval "$(atuin init zsh)"
    bindkey -M emacs '^x^r' atuin-ai-resume
    bindkey -M viins '^x^r' atuin-ai-resume-viins
    bindkey -M vicmd '^x^r' atuin-ai-resume-vicmd
    ```

=== "bash"

    ```shell
    eval "$(atuin init bash)"
    atuin-bind -m emacs      '\C-x\C-r' atuin-ai-resume-emacs
    atuin-bind -m vi-insert  '\C-x\C-r' atuin-ai-resume-viins
    atuin-bind -m vi-command '\C-x\C-r' atuin-ai-resume-vicmd
    ```

    `atuin-bind` is the helper `atuin init bash` defines (see
    [Key Binding](../configuration/key-binding.md)); it also works under ble.sh.

=== "fish"

    ```fish
    atuin init fish | source
    bind ctrl-x,ctrl-r _atuin_ai_resume
    bind -M insert ctrl-x,ctrl-r _atuin_ai_resume
    ```

    (Fish 3.x spells the key `\cx\cr`.)

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
