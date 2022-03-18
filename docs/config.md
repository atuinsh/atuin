# Config

Atuin maintains two configuration files, stored in `~/.config/atuin/`. We store
data in `~/.local/share/atuin` (unless overridden by XDG\_\*).

You can also change the path to the configuration directory by setting
`ATUIN_CONFIG_DIR`. For example

```
export ATUIN_CONFIG_DIR = /home/ellie/.atuin
```

## Client config

```
~/.config/atuin/config.toml
```

The client runs on a user's machine, and unless you're running a server, this
is what you care about.

See [config.toml](../atuin-client/config.toml) for an example

### `dialect`

This configures how the [stats](stats.md) command parses dates. It has two
possible values

```
dialect = "uk"
```

or

```
dialect = "us"
```

and defaults to "us".

### `auto_sync`

Configures whether or not to automatically sync, when logged in. Defaults to
true

```
auto_sync = true/false
```

### `sync_address`

The address of the server to sync with! Defaults to `https://api.atuin.sh`.

```
sync_address = "https://api.atuin.sh"
```

### `sync_frequency`

How often to automatically sync with the server. This can be given in a
"human readable" format. For example, `10s`, `20m`, `1h`, etc. Defaults to `1h`.

If set to `0`, Atuin will sync after every command. Some servers may potentially
rate limit, which won't cause any issues.

```
sync_frequency = "1h"
```

### `db_path`

The path to the Atuin SQlite database. Defaults to
`~/.local/share/atuin/history.db`.

```
db_path = "~/.history.db"
```

### `key_path`

The path to the Atuin encryption key. Defaults to
`~/.local/share/atuin/key`.

```
key = "~/.atuin-key"
```

### `session_path`

The path to the Atuin server session file. Defaults to
`~/.local/share/atuin/session`. This is essentially just an API token

```
key = "~/.atuin-session"
```

### `search_mode`

Which search mode to use. Atuin supports "prefix", full text and "fuzzy" search
modes. The prefix searches for "query\*", fulltext "\*query\*", and fuzzy applies
the search syntax [described below](#fuzzy-search-syntax).

Defaults to "prefix"

```
search_mode = "fulltext"
```

#### `fuzzy` search syntax

The "fuzzy" search syntax is based on the
[fzf search syntax](https://github.com/junegunn/fzf#search-syntax).

| Token     | Match type                 | Description                          |
| --------- | -------------------------- | ------------------------------------ |
| `sbtrkt`  | fuzzy-match                | Items that match `sbtrkt`            |
| `'wild`   | exact-match (quoted)       | Items that include `wild`            |
| `^music`  | prefix-exact-match         | Items that start with `music`        |
| `.mp3$`   | suffix-exact-match         | Items that end with `.mp3`           |
| `!fire`   | inverse-exact-match        | Items that do not include `fire`     |
| `!^music` | inverse-prefix-exact-match | Items that do not start with `music` |
| `!.mp3$`  | inverse-suffix-exact-match | Items that do not end with `.mp3`    |

A single bar character term acts as an OR operator. For example, the following
query matches entries that start with `core` and end with either `go`, `rb`,
or `py`.

```
^core go$ | rb$ | py$
```

## Server config

`// TODO`
