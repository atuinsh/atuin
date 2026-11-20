# Import existing history

Atuin uses a shell plugin to capture new shell history. But for older history, you will need to import it.

`atuin setup` imports the history for your current shell when it sets Atuin up. Setup reads the history file of zsh, bash, fish, or PowerShell. Setup skips the import when Atuin or your sync account already has history.

To import the history for your current shell yourself, run:

```
atuin import auto
```

Run `atuin import auto` if you skipped the import during setup, or to add this machine's history to an account that already has history.

Alternatively, you can specify the shell like so:

```
atuin import bash
atuin import zsh # etc
```

Other sources need `atuin import <source>`. For example, run `atuin import nu` for Nushell, or `atuin import zsh-hist-db` for a zsh-histdb database. See the [import reference](https://docs.atuin.sh/reference/import/index.md) for every source.

Your old shell history file will continue to be updated, regardless of Atuin usage.
