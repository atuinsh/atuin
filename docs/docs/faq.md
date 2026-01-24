# FAQ

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

We don't (yet) have a password reset system, as we don't verify emails. This
may change soon, but in the meantime so long as you're still logged in on at
least one account, it's safe to delete and re-create the account.

We're aware this isn't optimal.

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
