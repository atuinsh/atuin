# Key binding

По умолчанию, Autin будет переназначать <kbd>Ctrl-r</kbd> и клавишу 'стрелка вверх'.
Если вы не хотите этого, установите параметр ATUIN_NOBIND прежде чем вызывать `atuin init`

Например,

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"
```

Таким образом вы можете разрешить переназначение клавиш Autin, если это необходимо.
Делайте это до инициализирующего вызова.

# zsh

Autin устанавливает виджет ZLE "atuin-search"

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"

bindkey '^r' atuin-search

# зависит от режима терминала
bindkey '^[[A' atuin-search
bindkey '^[OA' atuin-search
```

# bash

```
export ATUIN_NOBIND="true"
eval "$(atuin init bash)"

# Переопределите  ctrl-r, и любые другие сочетания горячих клавиш тут
bind -x '"\C-r": __atuin_history'
```
