# 从历史记录中排除命令

有时你不希望某条命令出现在历史记录中，为此 Atuin 提供了四种排除命令的方法。

## 在命令前加一个空格

大多数 shell 都支持「ignorespace」这一约定：以行首空格开头输入的命令不会被保存到历史记录中。Atuin 遵循这一约定，这也是让单条命令不进入历史记录的最快方法。

```shell
 echo "this won't be saved"  # note the leading space
```

!!! warning "使用 bash-preexec 的 Bash"
    在使用 bash-preexec（而非 ble.sh）时存在一个已知问题：ignorespace 无法被完全遵循。该命令不会出现在 Atuin 中，但可能仍会保留在你的 bash 历史记录里。详情参见[安装](installation.md)。

## 按命令过滤：`history_filter`

[`history_filter`](../configuration/config.md#history_filter) 会排除任何匹配某个正则表达式的命令：

```toml
history_filter = [
    "^ls$",           # exclude bare 'ls', but not 'ls -la'
    "^cd ",           # exclude cd commands
    "--password",     # exclude anything with a password flag
]
```

这些模式并不锚定，因此 `secret` 会匹配命令中任意位置出现的该字符串。如果你想精确匹配整条命令，请使用 `^` 和 `$`。

## 按目录过滤：`cwd_filter`

[`cwd_filter`](../configuration/config.md#cwd_filter) 会排除所有在匹配目录下运行的命令：

```toml
cwd_filter = [
    "^/tmp",                    # nothing run from /tmp
    "/node_modules/",           # nothing run inside any node_modules
    "^/home/user/scratch",      # a scratch directory
]
```

这些模式同样是未锚定的正则表达式，会与工作目录路径进行匹配。

## 对某个工具完全跳过 Atuin

如果某个工具会启动交互式 shell，而你希望它完全不被记录，可以在 shell 配置文件中为 `atuin init` 调用加上判断条件：

```shell
# In .bashrc or .zshrc
if [[ -z "${MY_TOOL_SESSION}" ]]; then
    eval "$(atuin init bash)"
fi
```

然后配置该工具，使其在启动 shell 时设置 `MY_TOOL_SESSION=1`。关于该插件配置的其他调整方式，请参见 [`atuin init` 参考文档](../reference/init.md)。

!!! tip "来自 AI 代理的命令"
    你无需专门排除 AI 代理执行的命令来避免它们干扰你。Atuin 会为这些命令标注运行它们的代理，并默认在交互式搜索中将其隐藏——参见 [AI 代理 Hook](agent-hooks.md)。

## 清理已经记录下来的命令

过滤器只对此后新产生的记录生效。若要移除在你添加过滤器*之前*就已记录的条目，请运行 [`atuin history prune`](../reference/prune.md)：

```shell
# See what would be removed
atuin history prune --dry-run

# Remove it
atuin history prune
```

该操作会删除所有匹配当前 `history_filter` 和 `cwd_filter` 的现有条目。如果要删除不匹配任何过滤器的条目，请参见[删除历史记录](delete-history.md)。

## 敏感信息会被自动过滤

无论你是否配置了自定义过滤器，Atuin 默认都会拒绝记录那些疑似包含凭据的命令——例如 AWS 密钥、GitHub 和 npm 令牌、Slack webhook、Stripe 密钥等等。此功能默认开启。完整列表请参见 [`secrets_filter`](../configuration/config.md#secrets_filter)。
