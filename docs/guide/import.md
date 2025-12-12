# Import existing history

Atuin uses a shell plugin to ensure that we capture new shell history. But for
older history, you will need to import it

This will import the history for your current shell:
```
atuin import auto
```

Alternatively, you can specify the shell like so:

```
atuin import bash
atuin import zsh # etc
```

Your old shell history file will continue to be updated, regardless of Atuin usage.
