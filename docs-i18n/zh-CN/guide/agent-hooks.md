# AI 代理钩子

Atuin 可以捕获 AI 编程代理（例如 Claude Code、Codex 和 pi）运行的命令，并与你的常规 shell 历史记录一起保存。Atuin 会为每条命令标注运行它的代理，因此你可以按作者过滤历史记录。

## 快速开始

为你的代理安装钩子，然后重启或重新加载该代理：

```shell
# Claude Code
atuin hook install claude-code

# Codex
atuin hook install codex

# pi
atuin hook install pi
```

就是这样。代理运行的命令现在会出现在你的 Atuin 历史记录中，并标注该代理的名称。

## 工作原理

AI 编程代理支持钩子系统，可以在即将运行 shell 命令以及命令完成时通知外部工具。Atuin 正是利用这些钩子，把每条命令都记录为一条历史条目，如同你亲手输入的命令一样。

运行 `atuin hook install` 时，它会写入代理的配置文件或扩展，将 Atuin 注册为钩子处理程序：

| 代理 | 配置文件/扩展 |
|-------|-------------------------|
| Claude Code | `~/.claude/settings.json` |
| Codex | `~/.codex/hooks.json` |
| pi | `~/.pi/agent/extensions/atuin.ts` |

钩子生命周期如下：

1. **PreToolUse** —— 代理即将运行一条 Bash 命令。Atuin 会记录该命令、工作目录和时间戳（与 `history start` 相同）。
2. **PostToolUse / PostToolUseFailure** —— 命令已完成。Atuin 会记录退出代码和持续时间（与 `history end` 相同）。

Atuin 只捕获 `Bash` 工具的调用，会忽略其他工具类型（文件写入、网页抓取等）。

## 按作者过滤

默认情况下，Atuin 的交互式搜索只显示你自己的命令，代理运行的命令会被隐藏，以免干扰你的历史记录。

目前这一默认行为内置于搜索界面中，而非可以通过 `config.toml` 配置的选项。交互式搜索使用的等效值为：

- `$all-user` —— 任何**不是**已知 AI 代理的作者

如需显式指定作者过滤，可以使用 CLI 的 `atuin search --author ...` 标志。特殊值如下：

| 值 | 含义 |
|-------|---------|
| `$all-user` | 任何**不是**已知 AI 代理的作者 |
| `$all-agent` | 任何已知的 AI 代理作者 |

你也可以使用具体的作者名称：

```shell
# 只显示你自己的命令和 Claude Code 的命令
atuin search --author '$all-user' --author 'claude-code' -- ''
```

```shell
# 显示所有内容（不过滤）
atuin search -- ''
```

```shell
# 只显示代理的命令
atuin search --author '$all-agent' -- ''
```

目前已识别的代理名称有：`claude-code`、`codex`、`copilot`、`opencode` 和 `pi`。

## 支持的代理

支持级别请参阅[支持的平台](../support.md)。

### Claude Code

```shell
atuin hook install claude-code
```

这会在 `~/.claude/settings.json` 中添加钩子条目。Claude Code 每次使用 `Bash` 工具时都会调用 `atuin hook claude-code`，并通过 `stdin` 以 JSON 格式传递事件。

### Codex

```shell
atuin hook install codex
```

这会在 `~/.codex/hooks.json` 中添加钩子条目。Codex 每次匹配 `^Bash$` 的 Bash 工具使用都会调用 `atuin hook codex`。

### pi

```shell
atuin hook install pi
```

这会将 Atuin 的扩展写入 `~/.pi/agent/extensions/atuin.ts`。

然后重启 pi 或运行 `/reload`。该扩展会监听 pi 的工具事件，在命令执行前调用 `atuin history start`、执行后调用 `atuin history end`，从而将每条 `bash` 工具命令记录为作者 `pi`。由于它只是监听事件，而非注册自己的 `bash` 工具，因此可以与其他替换了 pi 的 bash 工具的扩展（例如沙盒或 RTK 实现）共同工作。

## 验证安装

安装钩子并重启代理后，通过该代理运行一条命令，然后检查你的历史记录：

```shell
# 显示包括代理命令在内的所有历史记录
atuin search --author '' -- ''

# 只显示代理命令
atuin search --author '$all-agent' -- ''
```

你也可以直接查看代理的配置文件，确认钩子已注册：

```shell
# Claude Code
cat ~/.claude/settings.json | grep atuin

# Codex
cat ~/.codex/hooks.json | grep atuin

# pi
ls ~/.pi/agent/extensions/atuin.ts
```

## 重新安装

再次运行 `atuin hook install` 是安全的。如果钩子已经安装，该命令会跳过它们并打印一条消息：

```
hooks.PreToolUse: already installed, skipping
hooks.PostToolUse: already installed, skipping
hooks.PostToolUseFailure: already installed, skipping
```

对于 pi，如果已托管的扩展与内置版本一致，重新安装时同样会跳过。
