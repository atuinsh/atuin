---
title: Listing History
---

# `atuin history list`


| Arg                 | Description                                                                   |
|---------------------|-------------------------------------------------------------------------------|
| `--cwd`/`-c`        | The directory to list history for (default: all dirs)                         |
| `--session`/`-s`    | Enable listing history for the current session only (default: false)          |
| `--human`           | Use human-readable formatting for the timestamp and duration (default: false) |
| `--cmd-only`        | Show only the text of the command (default: false)                            |
| `--reverse`      | Reverse the order of the output (default: false)                              |
| `--format`          | Specify the formatting of a command (see below)                               |

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
