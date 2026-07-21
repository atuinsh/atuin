# daemon

## `atuin daemon`

The Atuin daemon is a background daemon designed to

1. Speed up database writes
1. Allow machines to sync when not in use, so they're ready to go right away
1. Provides a hot in-memory fuzzy searcher
1. Perform background maintenance

It may also work around issues with ZFS/SQLite performance.

## To enable

Add the following to the bottom of your Atuin config file

```
[daemon]
enabled = true
autostart = true
```

With `autostart = true`, the CLI will automatically start and manage a local daemon for history hook calls. If you use systemd socket activation, keep `autostart = false`. If a legacy experimental daemon is already running, autostart cannot upgrade it in-place. Restart the daemon manually once.

If you prefer running the daemon yourself (for example via systemd/tmux), keep `autostart = false` and run `atuin daemon`.

## Extra config

See the [config section](https://docs.atuin.sh/configuration/config/#daemon)
