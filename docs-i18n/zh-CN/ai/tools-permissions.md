# Atuin AI 工具与权限

Atuin AI 配备了多种工具，可以在获得你的许可后与你的系统进行交互，用于帮助解答问题，并代表你执行操作。

## 权限系统

默认情况下，Atuin AI 在使用任何客户端工具之前都会先征求你的许可，你可以通过 _权限文件_ 修改这些默认设置。

### 权限文件

权限文件位于任意项目中的 `.atuin/permissions.ai.toml`。当 AI 想要运行某个工具时，Atuin AI 会先检查工作目录下是否存在 `.atuin/permissions.ai.toml` 文件，然后逐级检查所有父目录中的权限文件，直到文件系统根目录为止。最后，Atuin AI 还会检查 Atuin 配置目录（默认为 `~/.config/atuin/permissions.ai.toml`）中是否存在 `permissions.ai.toml` 文件。

权限文件是一个 TOML 文件，格式如下：

```toml
[permissions]

allow = [
    # 自动允许的工具规则
]

deny = [
    # 自动拒绝的工具规则
]

ask = [
    # 需要请求许可的工具规则
]
```

如果没有找到与之匹配的规则，Atuin AI 会默认先请求许可，然后再运行该工具。

在文件系统中，层级越深的权限文件优先级越高。例如，如果 Atuin AI 在当前工作目录中找到一条允许某工具的规则，即使父目录中的权限文件拒绝该工具，它仍然会允许该工具运行。

在同一个权限文件内部，`ask` 规则的优先级高于 `deny` 规则，`deny` 规则的优先级又高于 `allow` 规则。例如，如果某个权限文件中既有一条规则允许某个工具，又有一条规则要求为该工具请求许可，那么 Atuin AI 会在运行该工具之前先请求许可。

### 权限作用域

大多数规则都可以限定作用域，将其约束到特定的路径或其他上下文中。例如，你可以只允许 Atuin AI 读取某个特定目录中的文件，而不允许读取其他目录中的文件。对于文件操作相关的规则，作用域是一个用于匹配文件路径的 glob 模式。

### 配置示例

下面这个权限文件示例允许 Atuin AI 读取和写入当前项目中的任何 markdown 文件（因为 Write 隐含了 Read——见下文），但拒绝其访问任何 `.env` 文件。尝试读取或写入任何 _其他_ 文件都会导致 Atuin AI 在继续操作前请求许可。

```toml
[permissions]

allow = [
    "Write(**/*.md)"
]

deny = [
    "Read(.env)"
]
```

## 工具

### Atuin History（Atuin 历史记录）

`AtuinHistory` 工具允许 Atuin AI 搜索你的 Atuin 历史记录，以查找相关命令，该工具为只读。当你要求 AI 回忆过去运行过的命令或相关信息，或者请求帮助排查失败的命令时（例如「我上一条命令为什么失败了？」），Atuin AI 可能会请求使用此工具。

![Atuin History 工具示例](images/tool_atuin_history.png)

**权限规则与作用域：** `AtuinHistory`

**配置项：** `ai.capabilities.enable_history_search`（参见[设置文档](./settings.md#capabilities)）

**权限文件示例：**

```toml
[permissions]

allow = ["AtuinHistory"]
```

### Atuin Output（Atuin 输出）

`AtuinOutput` 工具允许 Atuin AI 读取你的 Atuin 历史记录中已捕获的命令输出，该工具为只读。当你询问某条已运行命令的结果，或请求帮助排查失败的命令时，Atuin AI 可能会请求使用此工具。输出捕获功能需要事先配置好守护进程和 pty-proxy——参见[读取命令输出](./command-output.md)。

**权限规则与作用域：** `AtuinOutput`

**配置项：** `ai.capabilities.enable_history_output`（参见[设置文档](./settings.md#capabilities)）

**权限文件示例：**

```toml
[permissions]

allow = ["AtuinOutput"]
```

### Read（读取）

`Read` 工具允许 Atuin AI 读取你系统上的文件。当你要求它分析某个文件的内容、请求编辑某个文件的内容，或者你提出的问题最适合通过查阅文件内容来回答时，Atuin AI 可能会请求使用此工具。

![Atuin 文件系统工具示例](images/tool_fs.png)

**权限规则与作用域：** `Read(<glob_pattern>)`（例如，`Read(**/*.md)` 允许读取当前目录及其子目录中的所有 markdown 文件）。如果缺少 glob 模式（例如仅写 `Read`），则匹配所有文件。

**配置项：** `ai.capabilities.enable_file_tools`（参见[设置文档](./settings.md#capabilities)）——此设置会同时启用 `Read` 和 `Write` 工具。

**权限文件示例：**

```toml
[permissions]
allow = ["Read(**/*.md)"]
deny = ["Read(.secret/**)"]
```

!!! warning "Write 隐含 Read"

    为防止意外的数据丢失，Atuin AI 在写入文件之前必须先读取该文件的内容。这意味着，任何允许对特定文件或一组文件使用 `Write` 工具的权限规则，也会自动允许对这些相同文件使用 `Read` 工具。例如，即使你没有明确允许 `Read(**/*.md)`，只要有一条允许 `Write(**/*.md)` 的规则，Atuin AI 也能够读取当前目录及其子目录中的任何 markdown 文件。

### Write（写入）

`Write` 工具允许 Atuin AI 在你的系统上创建和编辑文件。当你要求它更新某个工具的配置，或帮助调试某个问题时，Atuin AI 可能会请求使用此工具。

![Atuin 文件系统工具示例](images/tool_fs.png)

**权限规则与作用域：** `Write(<glob_pattern>)`（例如，`Write(**/*.md)` 允许写入当前目录及其子目录中的所有 markdown 文件）。如果缺少 glob 模式（例如仅写 `Write`），则匹配所有文件。

**配置项：** `ai.capabilities.enable_file_tools`（参见[设置文档](./settings.md#capabilities)）——此设置会同时启用 `Read` 和 `Write` 工具。

**权限文件示例：**

```toml
[permissions]
allow = ["Write(**/*.md)"]
deny = ["Write(.secret/**)"]
```

!!! note "文件备份"

    Atuin AI 在一次会话中首次写入某个文件时，会为原始文件创建一份备份，存储在 Atuin 数据目录下的 `ai/sessions/<session_id>` 中。该目录下有一个清单文件，用于将原始文件路径映射到备份文件路径。未来，我们会提供更便捷的方式，帮助你从意外的数据丢失中恢复。

### Shell 命令执行

`Shell` 工具允许 Atuin AI 在你的系统上执行 shell 命令。当你要求它执行某项最适合直接运行 shell 命令来完成的操作、请求帮助调试某个失败的命令，或是在多步骤工作流程中时，Atuin AI 可能会请求使用此工具。

![Atuin Shell 工具示例](images/tool_shell.png)

**权限规则与作用域：** `Shell(<command pattern>)`（例如，`Shell(git *)` 允许任何以 `git` 开头的命令）。如果缺少命令模式（例如仅写 `Shell`），则匹配所有命令。

**配置项：** `ai.capabilities.enable_command_execution`（参见[设置文档](./settings.md#capabilities)）

**权限文件示例：**

```toml
[permissions]
allow = [
    "Shell(git add *)",
    "Shell(git commit *)"
]
```

!!! note "命令执行的作用域"

    `Shell` 权限规则中的命令模式是根据命令中的各个词进行匹配的。`*` 通配符的行为会因其出现位置而异：

    | 模式 | 匹配 | 不匹配 |
    |---------|---------|----------------|
    | `*` | 任意命令 | — |
    | `git commit *` | `git commit`、`git commit -m "msg"` | `git`、`git push` |
    | `ls*` | `ls`、`ls -a`、`lsof` | `cat` |
    | `git * --amend` | `git commit --amend`、`git rebase --amend` | `git commit` |
    | `git commit` | `git commit` | `git`、`git push`、`git commit -m "msg"` |

    注意 `ls *`（带空格）与 `ls*`（不带空格）之间的区别。以空格分隔的形式使用**词边界**匹配——`ls *` 匹配 `ls` 和 `ls -a`，但_不_匹配 `lsof`。连写的形式则使用**前缀**匹配——`ls*` 会匹配上述所有情况，包括 `lsof`。

    对于 `allow` 和 `ask` 规则来说，不含任何通配符的模式（例如 `git commit`）属于**精确匹配**——只有当命令的各个词完全一致时才会匹配。如果你想允许 `git commit` 携带任意参数，请使用 `git commit *`。

    对于 `deny` 规则来说，不含任何通配符的模式（例如 `rm`）属于**前缀匹配**——它会匹配任何以该前缀开头的命令。这意味着 `rm` 的 `deny` 规则会同时拒绝 `rm`、`rm -rf /` 和 `rm ./README.md`，因此在编写不带显式通配符的 `deny` 规则时要格外小心。

!!! warning "复合命令"

    当 AI 运行一条复合命令时（例如 `git add . && npm test`），Atuin 会将其解析为多个独立的子命令。要让某条命令被自动允许，所有子命令都必须获得允许。这意味着 `git add . && npm test` 必须同时被 `Shell(git add *)` 和 `Shell(npm test)` 允许才会通过，否则它将无法匹配成功，转而请求许可。不过，我们的解析并不完美，可能存在一些边缘情况无法正确识别子命令，某些 shell 本身在命令解析上也表现欠佳。因此，我们建议在用宽泛的模式允许复合命令时保持谨慎。
