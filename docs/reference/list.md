# history list

## `atuin history list`


| Arg              | Description                                                                   |
|------------------|-------------------------------------------------------------------------------|
| `--cwd`/`-c`     | List history for the current directory only (default: all dirs)               |
| `--session`/`-s` | List history for the current session only (default: false)                    |
| `--human`        | Use human-readable formatting for the timestamp and duration (default: false) |
| `--cmd-only`     | Show only the text of the command (default: false)                            |
| `--reverse`      | Reverse the order of the output (default: false)                              |
| `--format`       | Specify the formatting of a command (see below)                               |
| `--print0`       | Terminate the output with a null, for better multiline support                                                                              |


## Format

Customize the output of `history list`

Example

```
atuin history list --format "{time} - {duration} - {command}"
```

Supported variables

```
{command}, {directory}, {duration}, {user}, {host} and {time}
```
