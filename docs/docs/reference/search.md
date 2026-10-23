# search

Atuin search supports wildcards, with either the `*` or `%` character. By
default, a prefix search is performed (that is, all queries are automatically
appended with a wildcard).

| Arg                  | Description |
| -------------------- | ----------------------------------------------------------------------------- |
| `--cwd`/`-c`         | The directory to list history for (default: all dirs)                         |
| `--exclude-cwd`      | Don't include commands that ran in this directory (default: none)             |
| `--exit`/`-e`        | Filter by exit code; repeat to include any listed code (default: none)        |
| `--exclude-exit`     | Exclude an exit code; repeat to exclude all listed codes (default: none)      |
| `--before`           | Only include commands run before this time (default: none)                    |
| `--after`            | Only include commands run after this time (default: none)                     |
| `--timezone`/`--tz`  | Timezone to display times in and to read `--before`/`--after` in (default: configured `timezone`) |
| `--interactive`/`-i` | Open the interactive search UI (default: false)                               |
| `--human`            | Use human-readable formatting for the timestamp and duration (default: false) |
| `--limit`            | Limit the number of results (default: none)                                   |
| `--offset`           | Offset from the start of the results (default: none)                          |
| `--delete`           | Delete history matching this query                                            |
| `--delete-it-all`    | Delete all shell history                                                      |
| `--reverse`          | Reverse order of search results, oldest first                                 |
| `--format`/`-f`      | Available variables: {command}, {directory}, {duration}, {user}, {host}, {time}, {exit} and {relativetime}. Example: --format "{time} - [{duration}] - {directory}$\t{command}" |
| `--inline-height`    | Set the maximum number of lines Atuin's interface should take up              |
| `--help`/`-h`        | Print help                                                                    |

## Dates for `--before` and `--after`

Both accept absolute dates (`2021-04-01`, `2021-04-01T15:00:00`) and relative
ones (`yesterday 3pm`, `2 hours ago`).

Slash-separated dates are read **day first**, so `01/04/2021` means 1 April
2021, not 4 January. This isn't configurable: the
[`dialect`](../configuration/config.md#dialect) setting applies to the
[`stats`](stats.md) command only. Prefer the unambiguous `YYYY-MM-DD` form.

## `atuin search -i`

Use Atuin's interactive search TUI to fuzzy search through your history.

![compact](https://user-images.githubusercontent.com/1710904/161623659-4fec047f-ea4b-471c-9581-861d2eb701a9.png)

You can replay the `nth` command with `alt + #` where `#` is the line number of the command you would like to replay.

Note: This isn't yet supported on macOS.

## Examples

```shell
# Open the interactive search TUI
atuin search -i

# Open the interactive search TUI preloaded with a query
atuin search -i atuin

# Search for all commands, beginning with cargo, that exited successfully
atuin search --exit 0 cargo

# Search for commands with exit code 1 or 2
atuin search --exit 1 --exit 2

# Exclude commands with exit code 0 or 130
atuin search --exclude-exit 0 --exclude-exit 130

# Search for all commands, that failed, from the current dir, and were ran before April 1st 2021
atuin search --exclude-exit 0 --before 2021-04-01 --cwd .

# Search for all commands, beginning with cargo, that exited successfully, and were ran after yesterday at 3pm
atuin search --exit 0 --after "yesterday 3pm" cargo

# Delete all commands, beginning with cargo, that exited successfully, and were ran after yesterday at 3pm
atuin search --delete --exit 0 --after "yesterday 3pm" cargo

# Search for a command beginning with cargo, return exactly one result.
atuin search --limit 1 cargo

# Search for a single result for a command beginning with cargo, skipping (offsetting) one result
atuin search --offset 1 --limit 1 cargo

# Find the oldest cargo command
atuin search --limit 1 --reverse cargo
```

When both flags are used, results must match one of the included codes and none of the excluded codes.
