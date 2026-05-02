# 键位绑定

默认情况下， Atuin 将会重新绑定 <kbd>Ctrl-r</kbd> 和 `up` 键。如果你不想使用默认绑定，请在调用 `atuin init` 之前设置 ATUIN_NOBIND

例如：

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"
```

如果需要，你可以在调用 `atuin init` 之后对 Atuin 重新进行键绑定

# zsh

Atuin 定义了 ZLE 部件 "atuin-search"

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"

bindkey '^r' atuin-search

# 取决于终端模式
bindkey '^[[A' atuin-search
bindkey '^[OA' atuin-search
```

# bash

```
export ATUIN_NOBIND="true"
eval "$(atuin init bash)"

# 绑定到 ctrl-r, 也可以在这里添加任何其他你想要的绑定方式
bind -x '"\C-r": __atuin_history'
```

# fish

```
set -gx ATUIN_NOBIND "true"
atuin init fish | source

# 在 normal 和 insert 模式下绑定到 ctrl-r，你也可以在此处添加其他键位绑定
bind \cr _atuin_search
bind -M insert \cr _atuin_search
```
