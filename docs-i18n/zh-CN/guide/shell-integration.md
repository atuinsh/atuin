# Shell 集成与互操作性

Atuin 使用 shell 钩子来捕获你的命令历史记录。本页说明这种集成的工作原理，以及为什么 Atuin 在某些环境中可能无法记录命令。

如果你想主动将特定命令排除在历史记录之外，请参阅[将命令排除在历史记录之外](excluding-commands.md)。

## Atuin 的 Shell 集成如何工作

当你在 shell 配置中添加 `eval "$(atuin init <shell>)"` 后，Atuin 会安装钩子，这些钩子会在 shell 命令生命周期的特定阶段运行：

1. **Preexec 钩子**：在每条命令执行*之前*运行。Atuin 会记录命令文本、时间戳和工作目录。
2. **Precmd 钩子**：在每条命令执行完成*之后*运行。Atuin 会记录退出代码和持续时间。

这些钩子只有在满足特定条件时才会生效：

- shell 必须是**交互式**的（以 `-i` 启动，或本身就是交互式的）
- 你的 shell 配置文件（`.bashrc`、`.zshrc` 等）必须被 **source**
- `atuin init` 命令必须在 shell 启动期间运行

如果以上任一条件不满足，Atuin 就不会安装其钩子，也就不会记录命令。

### 环境变量

Atuin 初始化时会设置以下几个环境变量：

| 变量 | 用途 |
|----------|---------|
| `ATUIN_SESSION` | 本次 shell 会话的唯一标识符 |
| `ATUIN_SHLVL` | 跟踪 shell 的嵌套层级 |
| `ATUIN_HISTORY_ID` | 当前正在执行命令的临时 ID |
| `ATUIN_HISTORY_AUTHOR` | 可选的命令作者身份（例如 `ellie`、`claude`、`copilot`） |
| `ATUIN_HISTORY_INTENT` | 可选的命令意图/理由文本 |

Atuin 在内部使用这些变量来跟踪命令的执行，并将命令与会话关联起来。
如果未设置 `ATUIN_HISTORY_AUTHOR`，Atuin 会默认使用本地 shell 的用户名。

## 内嵌终端与 IDE 集成

许多开发工具都内置了终端：

- **IDE**：PyCharm、IntelliJ、VS Code、Cursor、Zed
- **AI 编程助手**：Claude Code、GitHub Copilot CLI、Aider
- **容器环境**：Docker、Podman、devcontainers

这些工具启动 shell 的方式通常与你的常规终端不同，这可能会导致 Atuin 无法正常工作。

### 为什么 Atuin 可能无法工作

内嵌终端通常会：

1. **启动非交互式 shell**：许多工具通过 `bash -c "command"` 或类似方式运行命令，这不会触发 shell 配置的加载
2. **跳过 shell 配置**：出于性能或隔离性考虑，一些工具会显式避免 source `.bashrc`/`.zshrc`
3. **使用不同的 shell 路径**：内嵌终端使用的 shell 可能与你的默认 shell 不同

你可以运行以下命令来验证 Atuin 是否已激活：

```shell
atuin doctor
```

查看输出中的 `shell.preexec` 字段。如果显示为 `none`，说明 Atuin 的钩子未安装在该 shell 会话中。要确认 shell 是否为交互式，请检查 `echo $-` 的输出中是否包含 `i`。

### 在内嵌终端中启用 Atuin

如果你希望 Atuin 能够记录来自内嵌终端的命令，你需要确保它启动的是一个会 source 你的配置文件的交互式 shell。

#### IDE 终端设置

大多数 IDE 都允许你自定义其内置终端所使用的 shell 命令：

**PyCharm / IntelliJ：**

1. 前往 Settings → Tools → Terminal
2. 将 "Shell path" 修改为包含 `-i` 标志：
   - Linux/macOS：`/bin/bash -i` 或 `/bin/zsh -i`
   - 或者创建一个包装脚本（见下文）

**VS Code：**

添加到你的 `settings.json`（请将其中的 shell 替换为你实际使用的）：

```json
{
  "terminal.integrated.profiles.linux": {
    "bash": {
      "path": "/bin/bash",
      "args": ["-i"]
    }
  },
  "terminal.integrated.profiles.osx": {
    "zsh": {
      "path": "/bin/zsh",
      "args": ["-i"]
    }
  }
}
```

#### 包装脚本方案

对于不支持传入 shell 参数的工具，可以创建一个包装脚本：

```shell
#!/bin/bash
# 保存为 ~/bin/interactive-bash.sh 并 chmod +x
exec /bin/bash -i "$@"
```

然后将你的 IDE 配置为使用 `~/bin/interactive-bash.sh` 作为 shell 路径。

#### 验证修复效果

配置完成后，在你的 IDE 中打开一个新终端并运行：

```shell
atuin doctor | grep preexec
```

你应该会看到 `built-in`、`bash-preexec`、`blesh` 或类似的值——而不是 `none`。

## 各 Shell 的特殊说明

### Bash

Atuin 为 Bash 支持两种 preexec 后端：

- **ble.sh**（推荐）：功能齐全的行编辑器，计时准确，并且能正确支持 ignorespace
- **bash-preexec**：更简单，但在子 shell 和 ignorespace 方面存在一些限制

Shell 集成会显式检查交互模式：

```bash
if [[ $- != *i* ]]; then
    # 非交互模式，跳过初始化
    return
fi
```

### Zsh

Zsh 通过 `add-zsh-hook` 提供原生的钩子支持。这种集成方式简单直接，在交互式会话中能够可靠地工作。

### Fish

Fish 使用其事件系统（`fish_preexec` 和 `fish_postexec` 事件）。它也遵循 Fish 的私密模式——以 `fish --private` 运行的命令不会被记录。

### Nushell、xonsh 和 PowerShell

这些 shell 同样受支持。关于如何在各自环境中加载该插件，请参阅[安装](installation.md#installing-the-shell-plugin)。
