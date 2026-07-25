# 基本用法

现在你已经完成设置并可以正常运行，接下来快速了解一下如何最有效地使用 Atuin。

## Atuin 会记录什么？

在你工作时，Atuin 会记录：

1. 你运行的命令
2. 你运行该命令时所在的目录
3. 你运行命令的时间，以及运行所花费的时长
4. 命令的退出代码
5. 主机名以及运行该命令的用户
6. 你运行命令时所在的 shell 会话

## 打开并使用 TUI

你可以随时通过默认键位绑定——上箭头键或 `ctrl-r`——打开 TUI。

进入 TUI 后，按回车键可立即执行命令，或按 tab 键将其插入到你的 shell 中以便编辑。

在 TUI 中搜索时，你可以通过按 `ctrl-r` 循环切换[过滤模式](advanced-usage.md#filter-mode)来缩小搜索范围——包括完整历史记录、当前机器、当前目录、当前 git 仓库或当前 shell 会话。

更多选项请参阅[高级用法](advanced-usage.md)页面。

## 常见配置修改

完整的配置项列表请参阅[配置参考页面](../configuration/config.md)。

默认配置文件位于 `~/.config/atuin/config.toml`。

### 键位绑定

如果你想调整键位绑定，请参阅[键位绑定的完整说明页面](../configuration/key-binding.md)，其中包含大量可配置选项，包括在你不喜欢上箭头键行为时将其禁用。

### 按回车键运行

你可能更希望 Atuin 始终插入所选命令以供编辑。要配置此行为，请在配置文件中设置：

```toml
enter_accept = false
```

### 内嵌窗口

如果你觉得全屏 TUI 太占用空间或让人应接不暇，可以像下面这样调整它：

```toml
# height of the search window
# 搜索窗口的高度
inline_height = 40
```

你可能也会更喜欢紧凑型 UI 模式：

```toml
style = "compact"
```

### tmux 弹出窗口

如果你使用 tmux，Atuin 可以在当前窗格上方以浮动弹出窗口的形式打开搜索界面，而不是直接覆盖当前窗格：

```toml
[tmux]
enabled = true
```

有关尺寸设置和相关要求，请参阅 [`tmux` 配置参考](../configuration/config.md#tmux)。
