---
title: Import History
sidebar_position: 2
---

# `atuin import`

Atuin can import your history from your "old" history file

`atuin import auto` will attempt to figure out your shell (via \$SHELL) and run
the correct importer

Unfortunately these older files do not store as much information as Atuin does,
so not all features are available with imported data.

## Importing from history file at a non-default location

You can use the `--from-file` option to import from a file from any location.
This is useful when for example, you are migrating your settings to a new machine.

```bash
# Bash, for example
atuin import bash --from-file "/path/to/your/.bash_history"
```

Alternatively, you can set the environment variable `$HISTFILE` to achieve
the same effect.

```bash
HISTFILE="/path/to/your/.bash_history" atuin import bash
```
