# MCP 服务器

Atuin 内置了 [MCP（Model Context Protocol）](https://modelcontextprotocol.io/) 服务器，让 Claude Code、Cursor 等外部 AI 工具可以访问你的 shell 历史记录。你的智能体可以查找你之前运行过的命令、检查它们是否成功，并且在配置好[输出捕获](./command-output.md)之后，还能读取它们打印的内容。

该服务器所暴露的历史记录工具，与 [Atuin AI](./introduction.md) 使用的完全相同；两者均为只读——不会有任何操作能修改或删除你的历史记录，所有数据都保留在你自己的机器上。

## 启动服务器

MCP 服务器通过 stdio 运行，因此由你的 MCP 客户端负责启动它，无需在后台保持任何进程运行。命令如下：

```shell
atuin mcp
```

### Claude Code

```shell
claude mcp add atuin -- atuin mcp
```

### Cursor、Claude Desktop 及其他客户端

大多数 MCP 客户端接受如下的 JSON 配置：

```json
{
  "mcpServers": {
    "atuin": {
      "command": "atuin",
      "args": ["mcp"]
    }
  }
}
```

如果 `atuin` 二进制文件不在你的客户端的 `PATH` 中，请改用该二进制文件的完整路径（例如 `~/.atuin/bin/atuin`）。

## 工具

### `atuin_history`

搜索你的 shell 历史记录，使用与搜索 TUI 相同的模糊匹配方式。每条结果都包含命令本身、运行的时间和位置、退出代码、持续时间，以及一个可以传给 `atuin_output` 的历史记录 ID。

搜索可以通过以下几种方式缩小范围：

- **过滤模式**：与[交互式搜索](../guide/advanced-usage.md)相同的作用域——`global`、`host`、`directory`、`workspace` 或 `session`。`directory` 和 `workspace` 作用域相对于你的 MCP 客户端启动该服务器时所在的目录而言，对大多数编辑器来说，这就是你的项目目录。
- **仅失败的命令**：只返回退出代码非零的命令。
- **作者**：筛选出你自己运行的命令、由 AI 智能体运行的命令，或由某个特定智能体运行的命令。关于 Atuin 如何记录由智能体运行的命令，参见 [AI 智能体 Hooks](../guide/agent-hooks.md)。

历史记录搜索直接读取 Atuin 数据库，因此无需任何额外设置即可使用。

### `atuin_output`

获取此前某条命令所捕获的终端输出，以 `atuin_history` 结果中的历史记录 ID 作为标识。智能体可以只获取特定的行范围，因此不必为了查找末尾的错误信息而读取一份巨大的日志。

输出捕获需要 [daemon](../reference/daemon.md) 和 [pty-proxy](../reference/pty-proxy.md) 处于运行状态，设置方法参见[读取命令输出](./command-output.md)。如果它们没有运行，该工具会返回一条错误信息，说明没有可用的输出。

!!! note "会话作用域"

    只有当 MCP 服务器是从一个已启用 Atuin 的 shell 会话内部启动时，`session` 过滤模式才能正常工作。编辑器等客户端通常在这样的会话之外启动它，在这种情况下，其他过滤模式仍然可以照常使用。
