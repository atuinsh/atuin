# output

## `atuin output search`

Search the output of every captured command. Needs [output
capture](../guide/output-capture.md) set up; if the daemon isn't running and
`autostart` is on, Atuin starts it.

```shell
atuin output search <words>...
```

A command matches when every word appears in its output. Words match whole
words, ignoring case: `refused` matches `Refused` but `refuse` doesn't match
`refused`. Matches come back most relevant first, each with its command, when
and where it ran, and its output.

```shell
# The five most relevant captures mentioning both words
atuin output search connection refused

# Only the matching lines, with two lines either side
atuin output search -C 2 permission denied

# Use -- before a query that starts with a dash
atuin output search -- --force
```

Your own `atuin output search` runs are left out of the results, since their
output would match every query.

### `--limit <n>`

Default: `5`

The most matches to show.

### `--context <n>` / `-C <n>`

Show only the matching lines, with `n` lines of context either side. Without
it, each match shows its whole output.

### `--style <style>`

Default: `auto`

How matches are printed:

| Style | Output |
| ----- | ------ |
| `auto` | `pretty` on a terminal (unless `NO_COLOR` is set), `plain` otherwise |
| `plain` | Each command followed by its output lines, uncoloured |
| `pretty` | Each command with when it ran, how long it took, and its exit code, then its output with the matches highlighted |
| `json` | One JSON array of matches |
| `ndjson` | One JSON object per line, one per match |

Each JSON match has these fields:

| Field | Meaning |
| ----- | ------- |
| `id` | The history entry's ID |
| `timestamp_unix_ns` | When the command started, in nanoseconds since the Unix epoch |
| `command` | The command line |
| `cwd` | The directory it ran in |
| `session` | The shell session it ran in |
| `exit` | Its exit code |
| `duration_ns` | How long it ran, in nanoseconds |
| `output` | The matching output: the lines around each match with `--context`, otherwise all of it |
