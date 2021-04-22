# `atuin import`

Atuin can import your history from your "old" history file

`atuin import auto` will attempt to figure out your shell (via \$SHELL) and run
the correct importer

Unfortunately these older files do not store as much information as Atuin does,
so not all features are available with imported data.

# zsh

```
atuin import zsh
```

If you've set HISTFILE, this should be picked up! If not, try

```
HISTFILE=/path/to/history/file atuin import zsh
```

This supports both the simple and extended format

# bash

TODO
