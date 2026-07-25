# 常见问题解答

## 为什么 Atuin 没有记录我的 IDE 终端中的命令？

像 PyCharm、VS Code 这样的 IDE 通常会启动非交互式 shell，而这类 shell 不会加载你的 shell 配置文件。这就意味着 Atuin 的 hooks 从未被安装。

要解决这个问题，请将你的 IDE 配置为启动交互式 shell（例如使用 `/bin/bash -i` 而不是 `/bin/bash`）。

详细说明请参见 [Shell 集成与互操作性](guide/shell-integration.md)。

## 如何从历史记录中排除某些命令？

在 `~/.config/atuin/config.toml` 中使用 `history_filter` 选项：

```toml
history_filter = [
    "^secret-cmd",
    "^ls$",
]
```

你也可以使用 `cwd_filter` 按目录排除命令，或者在单条命令前加一个空格来排除它。

更多选项请参见 [从历史记录中排除命令](guide/excluding-commands.md)。

## 如何移除默认的上箭头键位绑定？

打开你的 shell 配置文件，找到包含 `atuin init` 的那一行。

添加 `--disable-up-arrow`，例如：

```shell
eval "$(atuin init zsh --disable-up-arrow)"
```

更多内容请参见 [键位绑定](configuration/key-binding.md)。

## 如何移除 Atuin AI 默认的问号键位绑定？

打开你的 shell 配置文件，找到包含 `atuin init` 的那一行。

添加 `--disable-ai`，例如：

```shell
eval "$(atuin init zsh --disable-ai)"
```

## 如何编辑一条命令，而不是立即执行它？

按 tab 键！默认情况下，enter 会执行命令，而 tab 会将其插入以供编辑。

你可以在配置文件（`~/.config/atuin/config.toml`）中加入 `enter_accept = false`，让 `enter` 键的行为变为编辑命令。

## 如何删除我的账户？

**注意：** 此命令不会提示确认。

```shell
atuin account delete
```

这将删除你的账户以及远程服务器上的所有历史记录，但不会删除你本地的数据。

## 我忘记密码了！该如何重置？

我们目前还没有密码重置系统。只要你至少还有一台机器仍处于登录状态，删除并重新创建账户就是安全的。

## 我没有设置同步，现在却不得不重装系统了！

如果你有 `~/.local/share/atuin` 的备份，可以按以下步骤导入：
1. 禁用 Atuin：注释掉 shell 集成那一行；例如在 bash 中，该行是 `eval "$(atuin init bash)"`
2. 将备份复制到 `~/.local/share/atuin`
3. 重新启用 Atuin
4. 设置同步！

## 替代项目

如果你不喜欢 Atuin，也许下面这些项目更适合你：

- https://github.com/ddworken/hishtory
  - 用 go 编写
  - 也提供同步历史记录功能
- https://github.com/cantino/mcfly
  - 使用一个小型本地神经网络进行搜索
  - 仅支持本地历史记录
