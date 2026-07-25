# init

## `atuin init <shell>`

为给定的 shell 打印对应的 shell 插件。对其输出执行 eval（evaluating）会将 Atuin 的钩子和键位绑定安装到你的会话中，因此应把这条命令放入 shell 的启动文件里，而不是手动运行。

```shell
atuin init zsh
```

关于你所用的 shell 具体应添加哪些语句，请参见[安装](../guide/installation.md#installing-the-shell-plugin) —— 不同 shell 的语法有所不同。

支持的 shell：`zsh`、`bash`、`fish`、`nu`、`xonsh`、`powershell`。各层级的具体含义请参见[支持的平台](../support.md)。

## 它配置了哪些内容

- **钩子（Hooks）**，用于记录每条命令及其退出代码和持续时间。请参见 [Shell 集成](../guide/shell-integration.md)。
- **键位绑定**，为 ++ctrl+r++ 和 ++up++ 方向键绑定相应功能，并将 ++question++ 绑定到 [Atuin AI](../ai/introduction.md)。
- **Dotfiles**（如果[已启用](../configuration/config.md#dotfiles)）—— 你同步的别名和环境变量都定义在这里。

## 标志

| 标志 | 说明 |
|------|-------------|
| `--disable-up-arrow` | 不绑定 ++up++ 方向键 |
| `--disable-ctrl-r` | 不绑定 ++ctrl+r++ |
| `--disable-ai` | 不将 ++question++ 绑定到 [Atuin AI](../ai/introduction.md) |

例如，要保留 ++ctrl+r++ 但不改动方向键：

```shell
eval "$(atuin init zsh --disable-up-arrow)"
```

## 环境变量

| 变量 | 作用 |
|----------|--------|
| `ATUIN_NOBIND` | 若设置为任意值，则不绑定任何按键，相当于传入了全部 `--disable-*` 标志。 |
| `ATUIN_NO_BUILTIN_PREEXEC` | 仅适用于 Bash。阻止 `atuin init bash` 自动加载其内置的 bash-preexec（Atuin >= 18.18.0）。 |

如果你想自行决定键位绑定，不绑定任何按键会很有用：

```shell
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"

bindkey '^r' atuin-search
```

关于各 shell 所暴露的 widget 与函数名称，请参见[键位绑定](../configuration/key-binding.md)；关于如何自定义 TUI *内部*的按键，请参见[高级键位绑定](../configuration/advanced-key-binding.md)。
