# daemon

## `atuin daemon`

The Atuin daemon is a background daemon designed to

1. Speed up database writes
2. Allow machines to sync when not in use, so they're ready to go right away
3. Provide a hot in-memory fuzzy searcher
4. Store [captured command output](../guide/output-capture.md)
5. Perform background maintenance

It may also work around issues with ZFS/SQLite performance.

## To enable

```shell
atuin setup --only --daemon
```

Setup sets `enabled = true` and `autostart = true` in the `[daemon]` section of
your config, and sets the search mode to
[`daemon-fuzzy`](../configuration/config.md#search_mode).
`atuin setup --only --no-daemon` turns the daemon off and stops it. On macOS,
setup also removes the launchd agent that autostart installed.

To enable the daemon by hand, add the following to the bottom of your Atuin
config file:

```toml
[daemon]
enabled = true
autostart = true
```

With `autostart = true`, the CLI will automatically start and manage a local daemon for history hook calls.
If you use systemd socket activation, keep `autostart = false`.
If a legacy experimental daemon is already running, autostart can't upgrade it in-place. Restart the daemon manually once.

If you prefer running the daemon yourself (for example via systemd/tmux), keep `autostart = false` and run `atuin daemon`.

## Extra config

See the [config section](../configuration/config.md#daemon)
