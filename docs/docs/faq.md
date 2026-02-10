# FAQ

## Why isn't Atuin recording commands in my IDE's terminal?

IDEs like PyCharm, VS Code, and others often start non-interactive shells that don't source your shell configuration. This means Atuin's hooks never get installed.

To fix this, configure your IDE to start an interactive shell (e.g., `/bin/bash -i` instead of `/bin/bash`).

See [Shell Integration and Interoperability](guide/shell-integration.md) for detailed instructions.

## How do I exclude certain commands from my history?

Use the `history_filter` option in `~/.config/atuin/config.toml`:

```toml
history_filter = [
    "^secret-cmd",
    "^ls$",
]
```

You can also exclude commands by directory with `cwd_filter`, or prefix individual commands with a space.

See [Shell Integration and Interoperability](guide/shell-integration.md#excluding-commands-from-history) for more options.

## How do I remove the default up arrow binding?

Open your shell config file, find the line containing `atuin init`.

Add `--disable-up-arrow`

EG:

```
eval "$(atuin init zsh --disable-up-arrow)"
```

See [key binding](../configuration/key-binding.md) for more

## How do I edit a command instead of running it immediately?

Press tab! By default, enter will execute a command, and tab will insert it ready for editing.

You can make `enter` edit a command by putting `enter_accept = false` into your config file (~/.config/atuin/config.toml)

## How do I delete my account?

**Attention:** This command does not prompt for confirmation.

```
atuin account delete
```

This will delete your account, and all history from the remote server. It will not delete your local data.

## I've forgotten my password! How can I reset it?

We don't currently have a password reset system. So long as you're still logged
in on at least one machine, it's safe to delete and re-create your account.

## I did not set up sync, and now I have to reinstall my system!

If you have a backup of `~/.local/share/atuin`, you can import it by:
1. disabling atuin by commenting out the shell integration, e.g. for bash it's `eval "$(atuin init bash)"`
2. copying the backup to `~/.local/share/atuin`
3. reenabling atuin
4. setting up sync!

## Alternative projects

If you dont like atuin, perhaps one of these works better for you:

- https://github.com/ddworken/hishtory
  - written in go
  - also provides sync'ed history
- https://github.com/cantino/mcfly
  - uses a small local neural network for search
  - only local history
