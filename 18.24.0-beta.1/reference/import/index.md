# import

## `atuin import`

Atuin can import your history from your "old" history file

`atuin import auto` will attempt to figure out your shell (via $SHELL) and run the correct importer

Unfortunately these older files don't store as much information as Atuin does, so not all features are available with imported data.

Except as noted otherwise, you can set the `HISTFILE` environment variable to control which file is read, otherwise each importer will try some default filenames.

```
HISTFILE=/path/to/history/file atuin import zsh
```

Note that for shells such as Xonsh that store history in many files rather than a single file, `$HISTFILE` should be set to the directory that holds those files.

For formats that don't store timestamps, timestamps will be generated starting at the current time plus 1ms for each additional command in the history.

Most importers will discard commands found that have invalid UTF-8.

## bash

This will read the history from `$HISTFILE` or `$HOME/.bash_history`.

Warnings will be issued if timestamps are found to be out of order, which could also happen when a history file starts without timestamps but later entries include them.

## fish

fish supports multiple history sessions, so the importer will default to the `fish` session unless the `fish_history` environment variable is set. The file to be read will be `{session}_history` in `$XDG_DATA_HOME/fish/` (or `$HOME/.local/share/fish`).

Not all of the data in the fish history is preserved, some data about filenames used for each command aren't used by Atuin, so it's discarded.

## nu

This importer reads from Nushell's text history format, which is stored in `$XDG_CONFIG_HOME/nushell/history.txt` or `$HOME/.config/nushell/history.txt`. The filename can't be set otherwise.

## nu-hist-db

This importer reads from Nushell's SQLite history database, which is stored in `$XDG_CONFIG_HOME/nushell/history.sqlite3` or `$HOME/.config/nushell/history.sqlite3`. The filename can't be set otherwise.

## `powershell`

This importer reads from [PowerShell's history file](https://learn.microsoft.com/en-us/powershell/module/psreadline/about/about_psreadline#command-history). On Windows, the file is located at `$APPDATA\Microsoft\Windows\PowerShell\PSReadLine\ConsoleHost_history.txt`. On other systems, it's located at `$XDG_DATA_HOME/powershell/PSReadLine/ConsoleHost_history.txt` or `$HOME/.local/share/powershell/PSReadLine/ConsoleHost_history.txt`.

## replxx

The [replxx](https://github.com/AmokHuginnsson/replxx) importer will read from `$HISTFILE` or `$HOME/.histfile`.

## resh

The [RESH](https://github.com/curusarn/resh) importer will read from `$HISTFILE` or `$HOME/.resh_history.json`.

## xonsh

The Xonsh importer will read from all JSON files it finds in the Xonsh history directory. The location of the directory is determined as follows: * If `$HISTFILE` is set, its value is used as the history directory. * If `$XONSH_DATA_DIR` is set (as it typically will be if the importer is invoked from within Xonsh), `$XONSH_DATA_DIR/history_json` is used. * If `$XDG_DATA_HOME` is set, `$XDG_DATA_HOME/xonsh/history_json` is used. * Otherwise, `$HOME/.local/share/xonsh/history_json` is used.

Not all data present in Xonsh history JSON files is used by Atuin. Xonsh stores the environment variables present when each session was initiated, but this data is discarded by Atuin. Xonsh optionally stores the output of each command. If present, this data is also ignored by Atuin.

## `xonsh-sqlite`

The Xonsh SQLite importer will read from the Xonsh SQLite history file. The history file's location is determined by the same process as the regular Xonsh importer, but with `history_json` replaced by `xonsh-history.sqlite`.

The Xonsh SQLite backend doesn't store environment variables, but like the JSON backend it can optionally store the output of each command. As with the JSON backend, if present this data will be ignored by Atuin.

## zsh

This will read the Zsh history from `$HISTFILE` or `$HOME/.zhistory` or `$HOME/.zsh_history` in either the basic or extended format.

## zsh-hist-db

This will read the Zsh histdb SQLite file from `$HISTDB_FILE` or `$HOME/.histdb/zsh-history.db`.
