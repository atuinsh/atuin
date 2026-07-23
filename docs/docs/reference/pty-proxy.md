# pty-proxy

Atuin pty-proxy is an experimental lightweight PTY proxy, providing new features without needing to replace your existing terminal or shell. It currently supports bash, zsh, fish, and nu.

!!! Note "Previously `atuin hex`"

    `atuin pty-proxy` is a replacement for the old `atuin hex` command. `atuin hex` still works for backward compatibility reasons, but will eventually be removed.

## TUI Rendering

The search TUI exposes a tradeoff: the UI is either in fullscreen alt-screen mode that takes over your terminal, or inline mode that clears your previous output. Neither is great.

With pty-proxy, the Atuin popup renders over the top of your previous output, but when it's closed we can restore the output successfully.

!!! tip "Already using tmux?"

    tmux can solve the same problem without pty-proxy: set
    [`[tmux] enabled = true`](../configuration/config.md#tmux) and the search UI
    opens in a popup above your pane, leaving the pane untouched.

## Capturing command output

Because pty-proxy sits between your terminal and your shell, it can also record
what each command printed. It reads the [OSC 133](https://gitlab.freedesktop.org/Per_Bothner/specifications/blob/master/proposals/prompts-data-model.md)
prompt markers your shell emits to tell where one command's output ends and the
next begins, then hands each captured block to the [daemon](daemon.md), which
holds it in memory against the command's Atuin history ID.

That capture is what lets AI tools see what actually happened, rather than
guessing from the command alone:

- [Atuin AI](../ai/introduction.md) can answer "why did that fail?" by reading
  the real error, via its `AtuinOutput` tool
- External agents such as Claude Code and Cursor can do the same through Atuin's
  [MCP server](../ai/mcp.md)

Output capture needs **both** pty-proxy and the daemon running. Nothing is
captured by default. See [Reading Command Output](../ai/command-output.md) for
setup, retention limits, and privacy.

## Initialization

Atuin pty-proxy needs to be initialized separately from your existing Atuin config. Place the init line shown below in your shell's init script, as high in the document as possible, _before_ your normal `atuin init` call.

=== "zsh"

    ```shell
    eval "$(atuin pty-proxy init zsh)"
    ```

=== "bash"

    ```shell
    eval "$(atuin pty-proxy init bash)"
    ```

=== "fish"

    Add

    ```shell
    atuin pty-proxy init fish | source
    ```

    to your `is-interactive` block in your `~/.config/fish/config.fish` file

=== "Nushell"

    Run in *Nushell*:

    ```shell
    mkdir ~/.local/share/atuin/
    atuin pty-proxy init nu | save -f ~/.local/share/atuin/pty-proxy-init.nu
    ```

    Add to `config.nu`, **before** the regular `atuin init`:

    ```shell
    source ~/.local/share/atuin/pty-proxy-init.nu
    ```
    Nushell's `source` command requires a static file path, so you must
    pre-generate the file.

---

If the `atuin` binary is not in your `PATH` by default, you should initialize pty-proxy as soon as it is set. For example, for a bash user with Atuin installed in `~/.atuin/bin/atuin`, a config file might look like this:

```bash
export PATH=$HOME/.atuin/bin:$PATH
eval "$(atuin pty-proxy init bash)"

# ... other shell configuration ...

eval "$(atuin init bash)"
```
