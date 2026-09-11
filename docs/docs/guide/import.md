# Import existing history

Atuin uses a shell plugin to capture new shell history. But for
older history, you will need to import it.

This will import the history for your current shell:
```shell
atuin import auto
```

Alternatively, you can specify the shell like so:

```shell
atuin import bash
atuin import zsh # etc
```

Your old shell history file will continue to be updated, regardless of Atuin usage.

For Zsh, `HISTFILE` must be exported for Atuin to see it. To pass your current
Zsh history path for a single import, run:

```shell
HISTFILE="${HISTFILE:?Set HISTFILE to your Zsh history file}" atuin import zsh
```

Without an exported `HISTFILE`, Atuin tries fallback filenames, which can select
an older history file. See the [Zsh import reference](../reference/import.md#zsh).
