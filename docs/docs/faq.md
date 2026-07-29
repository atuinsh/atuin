# FAQ

## Why is not Atuin recording commands in my IDE's terminal?

IDEs like PyCharm, VS Code, and others often start non-interactive shells that do not source your shell configuration. This means Atuin's hooks never get installed.

To fix this, configure your IDE to start an interactive shell (for example, `/bin/bash -i` instead of `/bin/bash`).

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

See [Excluding Commands from History](guide/excluding-commands.md) for more options.

## How do I remove the default up arrow binding?

Open your shell config file, find the line containing `atuin init`.

Add `--disable-up-arrow`. For example:

```shell
eval "$(atuin init zsh --disable-up-arrow)"
```

See [key binding](configuration/key-binding.md) for more

## How do I remove the default question mark binding for Atuin AI?

Open your shell config file, find the line containing `atuin init`.

Add `--disable-ai`. For example:

```shell
eval "$(atuin init zsh --disable-ai)"
```

## How do I edit a command instead of running it immediately?

Press tab! By default, enter will execute a command, and tab will insert it ready for editing.

You can make `enter` edit a command by putting `enter_accept = false` into your config file (`~/.config/atuin/config.toml`)

## How do I delete my account?

**Attention:** This command does not prompt for confirmation.

```shell
atuin account delete
```

This will delete your account, and all history from the remote server. It will not delete your local data.

## I've forgotten my password! How can I reset it?

We do not currently have a password reset system. As long as you are still logged
in on at least one machine, it is safe to delete and re-create your account.

## I am not using sync — why is Atuin connecting to `api.atuin.sh`?

That is the update checker. At most once per hour, Atuin checks
`https://api.atuin.sh` for the latest release, and lets you know if you are out
of date. It is a version lookup — no history or personal data is involved.

To turn it off, add this to `~/.config/atuin/config.toml`:

```toml
update_check = false
```

With the [update check](configuration/config.md#update_check) disabled and sync
not set up, Atuin makes no network requests of its own.

If you'd rather the code was not in the binary at all, build from source without
the default features:

```shell
cargo build --release --no-default-features --features client,daemon,clipboard
```

This compiles out the update checker, the sync commands, and AI.

## I did not set up sync, and now I have to reinstall my system!

If you have a backup of `~/.local/share/atuin`, you can import it by:
1. disabling Atuin by commenting out the shell integration; for example, for bash it is `eval "$(atuin init bash)"`
2. copying the backup to `~/.local/share/atuin`
3. reenabling Atuin
4. setting up sync!

## Alternative projects

If you do not like Atuin, perhaps one of these works better for you:

- https://github.com/ddworken/hishtory
  - written in go
  - also provides synced history
- https://github.com/cantino/mcfly
  - uses a small local neural network for search
  - only local history
