# daemon

## `atuin daemon`
_This is experimental!_

The Atuin daemon is a background daemon designed to

1. Speed up database writes
2. Allow machines to sync when not in use, so they're ready to go right away
3. Perform background maintenance

It may also work around issues with ZFS/SQLite performance.

It's currently experimental, but is safe to use with a little bit of setup

## To enable

Add the following to the bottom of your Atuin config file

```toml
[daemon]
enabled = true
autostart = true
```

With `autostart = true`, the CLI will automatically start and manage a local daemon for history hook calls.
If you use systemd socket activation, keep `autostart = false`.
If a legacy experimental daemon is already running, autostart cannot upgrade it in-place. Restart the daemon manually once.

If you prefer running the daemon yourself (for example via systemd/tmux), keep `autostart = false` and run `atuin daemon`.

## Extra config

See the [config section](../configuration/config.md#daemon)
