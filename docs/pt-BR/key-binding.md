# Vinculação de Teclas

Por padrão, o Atuin irá rebindar `Ctrl-r` e a tecla `up`. Se você não quiser usar as vinculações padrão, defina `ATUIN_NOBIND` antes de chamar `atuin init`.

Por exemplo:

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"
```

Você pode rebindar o Atuin após chamar `atuin init`, se necessário.

# zsh

O Atuin define o widget ZLE "atuin-search".

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"

bindkey '^r' atuin-search

# Dependendo do modo do terminal
bindkey '^[[A' atuin-search
bindkey '^[OA' atuin-search
```

# bash

```
export ATUIN_NOBIND="true"
eval "$(atuin init bash)"

# Vincula a ctrl-r, você também pode adicionar quaisquer outras vinculações que desejar aqui
bind -x '"\C-r": __atuin_history'
```

# fish

```
set -gx ATUIN_NOBIND "true"
atuin init fish | source

# Vincula a ctrl-r nos modos normal e de inserção, você também pode adicionar outras vinculações de teclas aqui
bind \cr _atuin_search
bind -M insert \cr _atuin_search
```
