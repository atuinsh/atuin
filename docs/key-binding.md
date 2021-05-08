# Key binding

By default, Atuin will rebind both Ctrl-r and the up arrow. If you do not want
this to happen, set ATUIN_NOBIND before the call to `atuin init`

For example

```
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
