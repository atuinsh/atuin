# Prečice na tastaturi

Zadano, Atuin će preusmeriti <kbd>Ctrl-r</kbd> i taster 'strelica nagore'.
Ako to ne želite, postavite parametar ATUIN_NOBIND pre nego što pozovete `atuin init`

Na primer,

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"
```

Na ovaj način možete dozvoliti preusmeravanje tastera u Atuin-u, ako je to neophodno.
Uradite to pre inicijalizacionog poziva.

# zsh

Atuin instalira ZLE widget "atuin-search"

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"

bindkey '^r' atuin-search

# zavisi od režima terminala
bindkey '^[[A' atuin-search
bindkey '^[OA' atuin-search
```

# bash

```
export ATUIN_NOBIND="true"
eval "$(atuin init bash)"

# Predefinišite ctrl-r i bilo koje druge kombinacije prečica ovde
bind -x '"\C-r": __atuin_history'
```
