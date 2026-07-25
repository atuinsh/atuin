# 集成

本页面介绍 Atuin 与 shell 插件及工具的集成方式。关于 Atuin 的 shell 钩子工作原理，以及在嵌入式终端（IDE、AI 编程助手等）中的故障排查信息，请参阅 [Shell 集成与互操作性](guide/shell-integration.md)。

## zsh-autosuggestions

Atuin 会自动将自身注册为一种[自动建议策略](https://github.com/zsh-users/zsh-autosuggestions#suggestion-strategy)。

如果你想覆盖此行为，请在 `.zshrc` 中的 `"$(atuin init zsh)"` 之后添加你自己的配置。

## zsh-vi-mode

如果你正在使用 [Zsh Vi Mode](https://github.com/jeffreytse/zsh-vi-mode)，可能需要在 `.zshrc` 中添加以下内容，以防止它覆盖 Atuin 的默认键位绑定：

```shell
# Append a command directly (after sourcing zvm)
zvm_after_init_commands+=(
  'eval "$(atuin init zsh)"'
)
```

## ble.sh 自动补全（Bash）

在 Bash 中加载 Atuin 集成时，如果 ble.sh 可用，Atuin 会自动为 ble.sh 的自动建议功能定义并注册一个自动补全源。

如果你想更改此行为，请在 `.bashrc` 中的 `eval "$(atuin init bash)"` 之后重写 shell 函数 `ble/complete/auto-complete/source:atuin-history`。

如果你不想使用 Atuin 的自动补全源，请在 `.bashrc` 中的 `eval "$(atuin init bash)"` 之后添加以下设置：

```shell
# bashrc (after eval "$(atuin init bash)")

ble/util/import/eval-after-load core-complete '
  ble/array#remove _ble_complete_auto_source atuin-history'
```

## 嵌入式终端与 IDE

在 IDE（如 PyCharm、VS Code）或 AI 编程助手（如 Claude Code）内置的嵌入式终端中，Atuin 可能无法开箱即用。这是因为这些工具通常会启动非交互式 shell，而非交互式 shell 不会加载你的 shell 配置。

有关解决方案和变通方法，请参阅 [Shell 集成与互操作性](guide/shell-integration.md)。
