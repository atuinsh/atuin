# 读取命令输出

Atuin AI 可以读取你运行过的命令的输出。问一句"为什么那条命令失败了？"，它就能查看实际的错误信息，而不是仅凭命令本身猜测。

Atuin 默认不会捕获输出，你需要设置两个组件：[daemon](../reference/daemon.md)（守护进程），负责在内存中保存最近的输出；以及 [pty-proxy](../reference/pty-proxy.md)，负责从你的终端捕获输出。

## 设置

### 1. 启用守护进程

在你的 Atuin 配置文件（默认是 `~/.config/atuin/config.toml`）中添加以下内容：

```toml
[daemon]
enabled = true
autostart = true
```

设置 `autostart = true` 后，Atuin 会为你启动并管理守护进程。如果你想自行运行守护进程（例如通过 systemd），请参阅[守护进程文档](../reference/daemon.md)。

### 2. 启用 pty-proxy

将 pty-proxy 的初始化行添加到你 shell 的初始化脚本中，尽量放在文件靠前的位置，并且要在你常规的 `atuin init` 调用之*前*：

=== "zsh"

    ```shell
    eval "$(atuin pty-proxy init zsh)"
    ```

=== "bash"

    ```shell
    eval "$(atuin pty-proxy init bash)"
    ```

=== "fish"

    添加

    ```shell
    atuin pty-proxy init fish | source
    ```

    到你 `~/.config/fish/config.fish` 文件中的 `is-interactive` 代码块里

=== "Nushell"

    在 *Nushell* 中运行：

    ```shell
    mkdir ~/.local/share/atuin/
    atuin pty-proxy init nu | save -f ~/.local/share/atuin/pty-proxy-init.nu
    ```

    在 `config.nu` 中添加以下内容，**要在**常规的 `atuin init` 之前：

    ```shell
    source ~/.local/share/atuin/pty-proxy-init.nu
    ```

更多细节请参阅 [pty-proxy 文档](../reference/pty-proxy.md)，包括在你的 shell 启动时如果 `atuin` 不在 `PATH` 中该怎么办。

### 3. 重启你的 shell

打开一个新终端（或重新加载你的 shell 配置）。从现在起，pty-proxy 会捕获你在该会话中运行的每条命令的输出，供 AI 使用。

如果想试试看，运行一条会失败的命令，然后按 `?` 键，问 Atuin AI 它为什么失败。它会请求使用 `AtuinOutput` 工具的权限，然后读取输出并作答。

## 工作原理

pty-proxy 位于终端与 shell 之间，利用 shell 的提示符标记来判断每条命令输出的起止位置，然后把捕获到的内容发送给守护进程；守护进程会将其与对应的 Atuin 历史记录 ID 一起保存在内存中。当 Atuin AI 想要查看某条命令打印了什么时，会向守护进程按历史记录 ID 请求该输出。

## 隐私与保留期限

捕获到的输出保存在你机器的内存中：

- 守护进程为每条命令最多保留 1MB 输出，每个 shell 会话最多保留最近 128 条命令（最多 32MB 输出）。
- 守护进程停止后，输出即会丢失，只有守护进程运行期间捕获到的命令才可用。

在 LLM 请求某条具体命令的输出之前，Atuin 不会向它发送任何内容；默认情况下，Atuin AI 还会先征得你的同意。

## 权限

输出获取由 `AtuinOutput` 权限规则控制——参阅[工具与权限](./tools-permissions.md)。要让 Atuin AI 读取命令输出而无需每次都询问：

```toml
[permissions]

allow = ["AtuinOutput"]
```

要完全关闭此功能，请在你的 Atuin 配置中将 `ai.capabilities.enable_history_output` 设为 `false`（参阅[设置文档](./settings.md#capabilities)）。

## 从其他 AI 工具读取输出

捕获到的输出不仅限于 Atuin AI 使用——Claude Code、Cursor 等外部工具也可以通过 Atuin 的 [MCP 服务器](./mcp.md)读取它。
