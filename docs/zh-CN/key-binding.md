# 键绑定

默认情况下， Atuin 将会重新绑定 <kbd>Ctrl-r</kbd> 和 `up` 键。如果你不想使用默认绑定，请在调用 `atuin init` 之前设置 ATUIN_NOBIND

例如：

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"
```

如果需要，你可以在调用 `atuin init` 之后对 Atuin 重新进行键绑定

# zsh

Atuin 定义了 ZLE 部件 "\_atuin_search_widget"

```
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"

bindkey '^r' _atuin_search_widget

# 取决于终端模式
bindkey '^[[A' _atuin_search_widget
bindkey '^[OA' _atuin_search_widget
```

# bash

```
export ATUIN_NOBIND="true"
eval "$(atuin init bash)"

# 绑定到 ctrl-r, 也可以在这里添加任何其他你想要的绑定方式
bind -x '"\C-r": __atuin_history'
```
