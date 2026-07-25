# AI 设置

控制 [Atuin AI](./introduction.md) 行为的所有设置都在 `config.toml` 的 `[ai]` 部分指定。有关 Atuin 配置系统的更多详细信息，请参阅[配置文档](../configuration/config.md)。

### enabled

默认值：`false`

是否启用 AI 功能。设置为 `false` 时，问号键位绑定会输出一条消息，说明如何运行 `atuin setup` 来启用该功能。

### model

默认值：未设置

新会话使用的 Atuin AI 模型。若未设置，将使用默认模型。你可以在 Atuin AI 界面中运行 `/model` 查看可用的模型。

### `db_path`

默认值：Atuin 数据目录中的 `ai_sessions.db`

存储 Atuin AI 会话的 SQLite 数据库路径。

### `session_continue_minutes`

默认值：`60`（分钟）

自与 Atuin AI 最后一次交互起，会话被视为「最近」、可以自动继续的时长。如果你与 Atuin AI 交互后，又在这个时间窗口内再次调用它，第二次交互会归入同一个会话；若间隔超出这个窗口，则会开始一个新会话。你也可以随时在 Atuin AI 界面中使用 `/new` 斜杠命令手动开始新会话。

### endpoint

默认值：`null`

Atuin AI 端点的地址，用于命令生成等 AI 功能。大多数用户无需设置此项，只有在使用自定义 AI 端点时才需要。

### `api_token`

默认值：`null`

Atuin AI 端点的 API 令牌，用于命令生成等 AI 功能。大多数用户无需设置此项，只有在使用自定义 AI 端点时才需要。

### `endpoint_protocol`

默认值：`"auto"`

客户端与配置的 `endpoint` 通信的方式。可选值为：

- `"auto"` —— 根据 `endpoint` 自动推断：官方 Atuin 地址使用 Hub 协议，其他地址则被视为 OSS 服务器。
- `"hub"` —— 将该端点视为 Atuin Hub 实例：通过基于浏览器的 Hub 流程登录，并上报积分使用情况。主要用于本地 Hub 实例的开发场景。
- `"oss"` —— 将该端点视为独立的 AI 服务器，例如 [`atuin-ai-server`](https://github.com/atuinsh/atuin-ai-server)。没有登录流程；若设置了 `api_token`，请求会用它进行身份验证。

在默认值 `"auto"` 下，只需将 `endpoint` 指向你自己的服务器即可直接生效：如果你的服务器需要令牌，设置 `api_token` 即可。

### `yolo`

默认值：`false`

启用 YOLO 模式，该模式会自动允许所有权限检查。**请谨慎使用此设置。**

此设置**不会**启用任何能力，只是跳过权限检查。

## 能力

控制哪些能力会被发送给 LLM 的设置，LLM 借此了解客户端具备哪些可用功能。这些设置在 `[ai.capabilities]` 下指定。

### `enable_history_search`

默认值：`true`

是否在发送给 LLM 的上下文中包含「历史记录搜索」能力。这样 AI 在生成建议或回答问题时，就可以请求搜索你的 Atuin 历史记录以查找相关命令。

### `enable_history_output`

默认值：`true`

是否在发送给 LLM 的上下文中包含「历史记录输出」能力。这样 AI 就可以请求查看之前命令的输出。这需要启用并运行 [pty-proxy](../reference/pty-proxy.md) 和[守护进程](../reference/daemon.md)，Atuin 才能捕获命令的输出——设置方法请参阅[读取命令输出](./command-output.md)。

### `enable_file_tools`

默认值：`true`

是否在发送给 LLM 的上下文中包含「文件工具」能力。这样 AI 就可以请求读取和更新你系统上的文件。

### `enable_command_execution`

默认值：`true`

是否在发送给 LLM 的上下文中包含「命令执行」能力。这样 AI 就可以请求在你的系统上执行命令。

**配置示例**

```toml
[ai.capabilities]
enable_history_search = false
```

## 初始上下文

控制在初始 AI 请求中发送哪些上下文的设置。这些设置在 `[ai.opening]` 下指定。

### `send_cwd`

默认值：`false`

是否在发送给 LLM 的上下文中包含你当前的工作目录。默认情况下，只会发送你的操作系统和当前 shell。

**配置示例**

```toml
[ai.opening]
send_cwd = true
```

### `send_last_command`

默认值：`false`

是否在初始请求中将你的上一条命令作为上下文发送，以便 AI 提供更贴切的建议。

**配置示例**

```toml
[ai.opening]
send_last_command = true
```
