# hex

Atuin Hex is an experimental lightweight PTY proxy, providing new features without needing to replace your existing terminal or shell. Atuin Hex currently supports bash, zsh, fish, and nu.

## TUI Rendering

The search TUI exposes a tradeoff: the UI is either in fullscreen alt-screen mode that takes over your terminal, or inline mode that clears your previous output. Neither is great.

With Hex, we can have our cake AND eat it too. The Atuin popup renders over the top of your previous output, but when it's closed we can restore the output successfully.

## Initialization

Atuin Hex needs to be initialized separately from your existing Atuin config. Place the init line shown below in your shell's init script, as high in the document as possible, *before* your normal `atuin init` call.

=== "zsh"

    ```shell
    eval "$(atuin hex init zsh)"
    ```

=== "bash"

    ```shell
    eval "$(atuin hex init bash)"
    ```

=== "fish"

    Add

    ```shell
    atuin hex init fish | source
    ```

    to your `is-interactive` block in your `~/.config/fish/config.fish` file

=== "Nushell"

    Run in *Nushell*:

    ```shell
    mkdir ~/.local/share/atuin/
    atuin hex init nu | save -f ~/.local/share/atuin/hex-init.nu
    ```

    Add to `config.nu`, **before** the regular `atuin init`:

    ```shell
    source ~/.local/share/atuin/hex-init.nu
    ```
    Nushell's `source` command requires a static file path, so you must
    pre-generate the file.

---

If the `atuin` binary is not in your `PATH` by default, you should initialize Hex as soon as it is set. For example, for a bash user with Atuin installed in `~/.atuin/bin/atuin`, a config file might look like this:

```bash
export PATH=$HOME/.atuin/bin:$PATH
eval "$(atuin hex init bash)"

# ... other shell configuration ...

eval "$(atuin init bash)"
```
