# Key binding

By default, Atuin will rebind both <kbd>Ctrl-r</kbd> and the up arrow.

You can also disable either the up-arrow or <kbd>Ctrl-r</kbd> bindings individually, by passing
`--disable-up-arrow` or `-disable-ctrl-r` to the call to `atuin init`:

```
# Bind ctrl-r but not up arrow
eval "$(atuin init zsh --disable-up-arrow)"

# Bind up-arrow but not ctrl-r
eval "$(atuin init zsh --disable-ctrl-r)"
```

If you do not want either key to be bound, either pass both `--disable` arguments, or set the
environment varuable `ATUIN_NOBIND` to any value before the call to `atuin init`:

```
## Do not bind any keys
# Either:
eval "$(atuin init zsh --disable-up-arrow --disable-ctrl-r)"

# Or:
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"
```

You can then choose to bind Atuin if needed, do this after the call to init.

# zsh

Atuin defines the ZLE widget "\_atuin_search_widget"

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"

bindkey '^r' _atuin_search_widget

# depends on terminal mode
bindkey '^[[A' _atuin_search_widget
bindkey '^[OA' _atuin_search_widget
```

# bash

```
export ATUIN_NOBIND="true"
eval "$(atuin init bash)"

# bind to ctrl-r, add any other bindings you want here too
bind -x '"\C-r": __atuin_history'
```

# fish

```
set -gx ATUIN_NOBIND "true"
atuin init fish | source

# bind to ctrl-r in normal and insert mode, add any other bindings you want here too
bind \cr _atuin_search
bind -M insert \cr _atuin_search
```
