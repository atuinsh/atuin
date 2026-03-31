# config

## `atuin config`

Read, write, and inspect Atuin configuration values. Atuin resolves
configuration from multiple sources (defaults, config file, environment
variables). The `config` command lets you see what is set where, and change
values in your `config.toml` without opening an editor.

## Subcommands

### `atuin config get <key>`

Print the value of a key as it appears in your config file.

```
$ atuin config get search_mode
fuzzy

$ atuin config get daemon
[daemon]
enabled = true
socket_path = "/tmp/atuin_daemon.sock"
```

If the key is not present in the config file, you'll see:

```
$ atuin config get enter_accept
(not set in config file)
```

#### `--resolved` / `-r`

Print the effective value after merging defaults, the config file, and
environment variable overrides:

```
$ atuin config get enter_accept --resolved
false
```

This also works for table keys, showing all resolved children as flat
dotted key=value pairs:

```
$ atuin config get logs --resolved
logs.ai.file = ai.log
logs.daemon.file = daemon.log
logs.dir = /home/user/.local/share/atuin/logs
logs.enabled = true
logs.level = info
logs.search.file = search.log
```

#### `--verbose` / `-v`

Show both the config file value and the resolved value side by side:

```
$ atuin config get enter_accept --verbose
Config file:
  (not set in config file)

Resolved:
  false
```

### `atuin config set <key> <value>`

Set a configuration value in your `config.toml`. The file's existing
formatting and comments are preserved.

```
$ atuin config set search_mode fuzzy
$ atuin config set daemon.enabled true
```

#### Type detection

By default, `set` matches the TOML type of the existing value when the key
is already in the config file. This prevents accidentally changing a string
like `"300"` into an integer `300`.

For new keys (not already in the file), `set` auto-detects the type:

| Value         | Detected type |
|---------------|---------------|
| `true`/`false`| boolean       |
| `42`, `-1`    | integer       |
| `3.14`        | float         |
| anything else | string        |

!!! warning "Scalar values only"
    `atuin config set` can only set config entries with scalar values; for tables or arrays, edit the config file manually.

#### `--type` / `-t`

Override type detection with an explicit type:

```
$ atuin config set sync_frequency 600 --type string
```

Possible values: `auto`, `string`, `boolean`, `integer`, `float`.

Setting a key that is currently a table will produce an error directing you
to use a dotted key instead:

```
$ atuin config set logs true
Error: 'logs' is a table; use a dotted key like 'logs.key' to set a value within it
```

### `atuin config print [key]`

Print configuration values from your config file in TOML format. Without a
key, prints the entire file. With a key, prints that section:

```
$ atuin config print daemon
[daemon]
enabled = true
socket_path = "/tmp/atuin_daemon.sock"
pidfile_path = "/tmp/atuin_daemon.pid"
autostart = false
```
