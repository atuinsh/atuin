# Config

Atuin maintains two configuration files, stored in `~/.config/atuin/`. We store
data in `~/.local/share/atuin` (unless overridden by XDG\_\*).

The full path to the config file would be `~/.config/atuin/config.toml`

The config location can be overridden with ATUIN_CONFIG_DIR

### `db_path`

Default: `~/.local/share/atuin/history.db`

The path to the Atuin SQLite database.

```toml
db_path = "~/.history.db"
```

### `key_path`

Default: `~/.local/share/atuin/key`

The path to the Atuin encryption key.

```toml
key_path = "~/.atuin-key"
```

### `session_path`

Default: `~/.local/share/atuin/session`

The path to the Atuin server session file.
This is essentially just an API token

```toml
session_path = "~/.atuin-session"
```

### `dialect`

Default: `us`

This configures how the [stats](../reference/stats.md) command parses dates. It has two
possible values

```toml
dialect = "uk"
```

or

```toml
dialect = "us"
```

### `auto_sync`

Default: `true`

Configures whether or not to automatically sync, when logged in.

```toml
auto_sync = true/false
```

### `update_check`

Default: `true`

Configures whether or not to automatically check for updates.

```toml
update_check = true/false
```

### `sync_address`

Default: `https://api.atuin.sh`

The address of the server to sync with!

```toml
sync_address = "https://api.atuin.sh"
```

### `sync_frequency`

Default: `1h`

How often to automatically sync with the server. This can be given in a
"human-readable" format. For example, `10s`, `20m`, `1h`, etc.

If set to `0`, Atuin will sync after every command. Some servers may potentially
rate limit, which won't cause any issues.

```toml
sync_frequency = "1h"
```

### `search_mode`

Default: `fuzzy`

Which search mode to use. Atuin supports "prefix", "fulltext", "fuzzy", and
"skim" search modes.

Prefix mode searches for "query\*"; fulltext mode searches for "\*query\*";
"fuzzy" applies the [fuzzy search syntax](#fuzzy-search-syntax);
"skim" applies the [skim search syntax](https://github.com/lotabout/skim#search-syntax).

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

### `filter_mode`

Default: `global`

The default filter to use when searching

| Mode             | Description                                                                          |
|------------------|--------------------------------------------------------------------------------------|
| global (default) | Search from the full history                                                         |
| host             | Search history from this host                                                        |
| session          | Search history from the current session                                              |
| directory        | Search history from the current directory                                            |
| workspace        | Search history from the current git repository                                       |
| session-preload  | Search from the current session and the global history from before the session start |

Filter modes can still be toggled via ctrl-r

```toml
filter_mode = "host"
```

### `search_mode_shell_up_key_binding`

Atuin version: >= 17.0

Default: `fuzzy`

The default searchmode to use when searching and being invoked from a shell up-key binding.

Accepts exactly the same options as `search_mode` above

```toml
search_mode_shell_up_key_binding = "fuzzy"
```

Defaults to the value specified for `search_mode`.

### `filter_mode_shell_up_key_binding`

Default: `global`

The default filter to use when searching and being invoked from a shell up-key binding.

Accepts exactly the same options as `filter_mode` above

```toml
filter_mode_shell_up_key_binding = "session"
```

Defaults to the value specified for `filter_mode`.

### `workspaces`

Atuin version: >= 17.0

Default: `false`

This flag enables a pseudo filter-mode named "workspace": the filter is automatically
activated when you are in a git repository.

With workspace filtering enabled, Atuin will filter for commands executed in any directory
within a git repository tree.

Filter modes can still be toggled via ctrl-r.

### `style`

Default: `compact`

Which style to use. Possible values: `auto`, `full` and `compact`.

- `compact`:

![compact](https://user-images.githubusercontent.com/1710904/161623659-4fec047f-ea4b-471c-9581-861d2eb701a9.png)

- `full`:

![full](https://user-images.githubusercontent.com/1710904/161623547-42afbfa7-a3ef-4820-bacd-fcaf1e324969.png)

This means that Atuin will automatically switch to `compact` mode when the terminal window is too short for `full` to display properly.

### `invert`

Atuin version: >= 17.0

Default: `false`

Invert the UI - put the search bar at the top.

```toml
invert = true/false
```

### `inline_height`

Default: `40`

Set the maximum number of lines Atuin's interface should take up.

If set to `0`, Atuin will always take up as many lines as available (full screen).

### `show_preview`

Default: `true`

Configure whether or not to show a preview of the selected command.

Useful when the command is longer than the terminal width and is cut off.

### `max_preview_height`

Atuin version: >= 17.0

Default: `4`

Configure the maximum height of the preview to show.

Useful when you have long scripts in your history that you want to distinguish by more than the first few lines.

### `show_help`

Atuin version: >= 17.0

Default: `true`

Configure whether or not to show the help row, which includes the current Atuin version (and whether an update is available), a keymap hint, and the total amount of commands in your history.

### `show_tabs`

Atuin version: >= 18.0

Default: `true`

Configure whether or not to show tabs for search and inspect.

### `auto_hide_height`

Atuin version: >= 18.4

Default: `8`

Set Atuin to hide lines when a minimum number of rows is subceeded. This has no effect except
when `compact` style is being used (see `style` above), and currently applies to only the
interactive search and inspector. It can be turned off entirely by setting to `0`.

### `exit_mode`

Default: `return-original`

What to do when the escape key is pressed when searching

| Value                     | Behaviour                                                        |
| ------------------------- | ---------------------------------------------------------------- |
| return-original (default) | Set the command-line to the value it had before starting search  |
| return-query              | Set the command-line to the search query you have entered so far |

Pressing ctrl+c or ctrl+d will always return the original command-line value.

```toml
exit_mode = "return-query"
```

### `history_format`

Default to `history list`

The history format allows you to configure the default `history list` format - which can also be specified with the --format arg.

The specified --format arg will prioritize the config when both are present

More on [history list](../reference/list.md)

### `history_filter`

The history filter allows you to exclude commands from history tracking - maybe you want to keep ALL of your `curl` commands totally out of your shell history, or maybe just some matching a pattern.

This supports regular expressions, so you can hide pretty much whatever you want!

```toml
## Note that these regular expressions are unanchored, i.e. if they don't start
## with ^ or end with $, they'll match anywhere in the command.
history_filter = [
   "^secret-cmd",
   "^innocuous-cmd .*--secret=.+"
]
```

### `cwd_filter`

The cwd filter allows you to exclude directories from history tracking.

This supports regular expressions, so you can hide pretty much whatever you want!

```toml
## Note that these regular expressions are unanchored, i.e. if they don't start
## with ^ or end with $, they'll match anywhere in the command.
# cwd_filter = [
#   "^/very/secret/directory",
# ]
```

After updating that parameter, you can run [the prune command](../reference/prune.md) to remove old history entries that match the new filters.

### `store_failed`

Atuin version: >= 18.3.0

Default: `true`

```toml
store_failed = true
```

Configures whether to store commands that failed (those with non-zero exit status) or not.

### `secrets_filter`

Atuin version: >= 17.0

Default: `true`

```toml
secrets_filter = true
```

This matches history against a set of default regex, and will not save it if we get a match. Defaults include

1. AWS key id
2. Github pat (old and new)
3. Slack oauth tokens (bot, user)
4. Slack webhooks
5. Stripe live/test keys
6. Atuin login command
7. Cloud environment variable patterns (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AZURE_STORAGE_CLASS_KEY`, `GOOGLE_SERVICE_ACCOUNT_KEY`)
8. Netlify authentication tokens
9. Npm pat
10. Pulumi pat

### macOS Ctrl-n key shortcuts

Default: `true`

macOS does not have an ++alt++ key, although terminal emulators can often be configured to map the ++option++ key to be used as ++alt++. _However_, remapping ++option++ this way may prevent typing some characters, such as using ++option+3++ to type `#` on the British English layout. For such a scenario, set the `ctrl_n_shortcuts` option to `true` in your config file to replace ++alt+0++ to ++alt+9++ shortcuts with ++ctrl+0++ to ++ctrl+9++ instead:

```toml
# Use Ctrl-0 .. Ctrl-9 instead of Alt-0 .. Alt-9 UI shortcuts
ctrl_n_shortcuts = true
```

### `network_timeout`

Atuin version: >= 18.0

Default: `30`

The max amount of time (in seconds) to wait for a network request. If any
operations with a sync server take longer than this, the code will fail -
rather than wait indefinitely.

### `network_connect_timeout`

Atuin version: >= 18.0

Default: `5`

The max time (in seconds) we wait for a connection to become established with a
remote sync server. Any longer than this and the request will fail.

### `local_timeout`

Atuin version: >= 18.0

Default: `5`

Timeout (in seconds) for acquiring a local database connection (sqlite).

### `command_chaining`

Atuin version: >= 18.8

Default: `false`

Allows building a command chain with the `&&` or `||` operator. When enabled, opening atuin will search for the next command in the chain, and append to the current buffer.

### `enter_accept`

Atuin version: >= 17.0

Default: `false`

When set to true, Atuin will default to immediately executing a command rather
than the user having to press enter twice. Pressing tab will return to the
shell and give the user a chance to edit.

This technically defaults to true for new users, but false for existing. We
have set `enter_accept = true` in the default config file. This is likely to
change to be the default for everyone in a later release.

### `keymap_mode`

Atuin version: >= 18.0

Default: `emacs`

The initial keymap mode of the interactive Atuin search (e.g. started by the
keybindings in the shells). There are four supported values: `"emacs"`,
`"vim-normal"`, `"vim-insert"`, and `"auto"`. The keymap mode `"emacs"` is the
most basic one. In the keymap mode `"vim-normal"`, you may use ++k++
and ++j++ to navigate the history list as in Vim, whilst pressing

++i++ changes the keymap mode to `"vim-insert"`. In the keymap mode `"vim-insert"`,
you can search for a string as in the keymap mode `"emacs"`, while pressing ++esc++
switches the keymap mode to `"vim-normal"`. When set to `"auto"`, the initial
keymap mode is automatically determined based on the shell's keymap that triggered
the Atuin search. `"auto"` is not supported by NuShell at present, where it will
always trigger the Atuin search with the keymap mode `"emacs"`.

### `keymap_cursor`

Atuin version: >= 18.0

Default: `(empty dictionary)`

The terminal's cursor style associated with each keymap mode in the Atuin
search. This is specified by a dictionary whose keys and values being the
keymap names and the cursor styles, respectively. A key specifies one of the
keymaps from `emacs`, `vim_insert`, and `vim_normal`. A value is one of the
cursor styles, `default` or `{blink,steady}-{block,underline,bar}`. The
following is an example.

```toml
keymap_cursor = { emacs = "blink-block", vim_insert = "blink-block", vim_normal = "steady-block" }
```

If the cursor style is specified, the terminal's cursor style is changed to the
specified one when the Atuin search starts with or switches to the
corresponding keymap mode. Also, the terminal's cursor style is reset to the
one associated with the keymap mode corresponding to the shell's keymap on the
termination of the Atuin search.

### `prefers_reduced_motion`

Atuin version: >= 18.0

Default: `false`

Enable this, and Atuin will reduce motion in the TUI as much as possible. Users
with motion sensitivity can find the live-updating timestamps distracting.

Alternatively, set env var NO_MOTION

## search

### `filters`

Atuin version: >= 18.4

The list of filter modes available in interactive search, in the order they cycle through when you press ctrl-r. By default, all modes are enabled. Removing a mode from this list disables it entirely. The `workspace` mode is skipped when not in a git repository or when `workspaces = false`. See [`filter_mode`](#filter_mode) for a description of each mode.

The `filter_mode` setting selects the initial mode from this list. If `filter_mode` is set to a mode not in the list, the first available mode is used instead.

```toml
[search]
filters = ["global", "host", "session", "directory"]
```

## Stats

This section of client config is specifically for configuring Atuin stats calculations

```
[stats]
common_subcommands = [...]
common_prefix = [...]
```

### `common_subcommands`

Default:

```toml
common_subcommands = [
  "apt",
  "cargo",
  "composer",
  "dnf",
  "docker",
  "git",
  "go",
  "ip",
  "jj",
  "kubectl",
  "nix",
  "nmcli",
  "npm",
  "pecl",
  "pnpm",
  "podman",
  "port",
  "systemctl",
  "tmux",
  "yarn",
]
```

Configures commands where we should consider the subcommand as part of the statistics. For example, consider `kubectl get` rather than just `kubectl`.

### `common_prefix`

Atuin version: >= 17.1

Default:

```toml
common_prefix = [
  "sudo",
]
```

Configures commands that should be totally stripped from stats calculations. For example, 'sudo' should be ignored.

## sync

We have developed a new version of sync, that is both faster and more efficient than the original version.

Presently, it is the default for fresh installs but not for existing users. This will change in a later release.

To enable sync v2, add the following to your config

```toml
[sync]
records = true
```

## `dotfiles`

Atuin version: >= 18.1

Default: `false`

To enable sync of shell aliases between hosts. Requires `sync` enabled.

Add the new section to the bottom of your config file, for every machine you use Atuin with

```toml
[dotfiles]
enabled = true
```

Note: you will need to have sync v2 enabled. See the above section.

Manage aliases using the command line options

```
# Alias 'k' to 'kubectl'
atuin dotfiles alias set k kubectl

# List all aliases
atuin dotfiles alias list

# Delete an alias
atuin dotfiles alias delete k
```

After setting an alias, you will either need to restart your shell or source the init file for the change to take affect

## keys

This section of the client config is specifically for configuring key-related settings.

```
[keys]
scroll_exits = [...]
prefix = 'a'
```

### `scroll_exits`

Atuin version: >= 18.1

Default: `true`

Configures whether the TUI exits, when scrolled past the last or first entry.

### `prefix`

Atuin version: > 18.3

Default: `a`

Which key to use as the prefix

### `exit_past_line_start`

Atuin version: >= 18.5

Default: `true`

Exits the TUI when scrolling left while the cursor is at the start of the line.

### `accept_past_line_end`

Atuin version: >= 18.5

Default: `true`

The right arrow key performs the same functionality as Tab and copies the selected line to the command line to be
modified.

### `accept_past_line_start`

Atuin version: >= 18.9

Default: `false`

The left arrow key performs the same functionality as Tab and copies the selected line to the command line to be
modified.

### `accept_with_backspace`

Atuin version: >= 18.9

Default: `false`

The backspace key performs the same functionality as Tab and copies the selected line to the command line to be
modified.

## preview

This section of the client config is specifically for configuring preview-related settings.
(In the future the other 2 preview settings will be moved here.)

```
[preview]
strategy = [...]
```

### `strategy`

Atuin version: >= 18.3

Default: `auto`

Which preview strategy is used to calculate the preview height. It respects `max_preview_height`.

| Value          | Preview height is calculated from the length of the |
| -------------- | --------------------------------------------------- |
| auto (default) | selected command                                    |
| static         | longest command in the current result set           |
| fixed          | use `max_preview_height` as fixed value             |

By using `auto` a preview is shown, if the command is longer than the width of the terminal.

## Daemon

Atuin version: >= 18.3

Default: `false`

Enable the background daemon

Add the new section to the bottom of your config file

```toml
[daemon]
enabled = true
```

### autostart

Default: `false`

Automatically start and manage the daemon when needed.
This is not compatible with `systemd_socket = true`.
If a legacy experimental daemon is already running, restart it manually once before using autostart.

```toml
autostart = false
```

### sync_frequency

Default: `300`

How often the daemon should sync, in seconds

```toml
sync_frequency = 300
```

### socket_path

Default:

```toml
socket_path = "~/.local/share/atuin/atuin.sock"
```

Where to bind a unix socket for client -> daemon communication

If XDG_RUNTIME_DIR is available, then we use this directory instead.

### pidfile_path

Default:

```toml
pidfile_path = "~/.local/share/atuin/atuin-daemon.pid"
```

Path to the daemon pidfile used for process coordination.

### systemd_socket

Default `false`

Use a socket passed via systemd socket activation protocol instead of the path

```toml
systemd_socket = false
```

### tcp_port

Default: `8889`

The port to use for client -> daemon communication. Only used on non-unix systems.

```toml
tcp_port = 8889
```

## theme

Atuin version: >= 18.4

The theme to use for showing the terminal interface.

```toml
[theme]
name = "default"
debug = false
max_depth = 10
```

### `name`

Default: `"default"`

A theme name that must be present as a built-in (unset or `default` for the default,
else `autumn` or `marine`), or found in the themes directory, with the suffix `.toml`.
By default this is `~/.config/atuin/themes/` but can be overridden with the
`ATUIN_THEME_DIR` environment variable.

```toml
name = "my-theme"
```

### `debug`

Default: `false`

Output information about why a theme will not load. Independent from other log
levels as it can cause data from the theme file to be printed unfiltered to the
terminal.

```toml
debug = false
```

### `max_depth`

Default: 10

Number of levels of "parenthood" that will be traversed for a theme. This should not
need to be added in or changed in normal usage.

```toml
max_depth = 10
```

## ui

Atuin version: >= 18.5

Configure the interactive search UI appearance.

```toml
[ui]
columns = ["duration", "time", "command"]
```

### `columns`

Default: `["duration", "time", "command"]`

Columns to display in the interactive search, from left to right. The selection
indicator (`" > "`) is always shown first implicitly.

Each column can be specified as:
- A simple string (uses default width): `"duration"`
- An object with type and optional width/expand: `{ type = "directory", width = 30 }`

#### Available column types

| Column    | Default Width | Description                                     |
| --------- | ------------- | ----------------------------------------------- |
| duration  | 5             | Command execution duration (e.g., "123ms")      |
| time      | 8             | Relative time since execution (e.g., "59m ago") |
| datetime  | 16            | Absolute timestamp (e.g., "2025-01-22 14:35")   |
| directory | 20            | Working directory (truncated if too long)       |
| host      | 15            | Hostname where command was run                  |
| user      | 10            | Username                                        |
| exit      | 3             | Exit code (colored by success/failure)          |
| command   | *             | The command itself (expands by default)         |

#### Column options

- **type**: The column type (required when using object format)
- **width**: Custom width in characters (optional, uses default if not specified)
- **expand**: If `true`, the column fills remaining space. Default is `true` for `command`, `false` for others. Only one column should have `expand = true`.

#### Examples

```toml
# Minimal - more space for commands
columns = ["duration", "command"]

# With custom directory width
columns = ["duration", { type = "directory", width = 30 }, "command"]

# Show host for multi-machine sync users
columns = ["duration", "time", "host", "command"]

# Show exit codes prominently
columns = ["exit", "duration", "command"]

# Make directory expand instead of command
columns = ["duration", "time", { type = "directory", expand = true }, { type = "command", expand = false }]
```

## ai

The settings for Atuin AI are listed in [a separate section](../../ai/settings/).
