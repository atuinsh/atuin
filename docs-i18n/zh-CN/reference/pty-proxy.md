# pty-proxy

Atuin pty-proxy 是一个实验性的轻量级 PTY 代理，无需替换你现有的终端或 shell，即可提供新功能，目前支持 bash、zsh、fish 和 nu。支持级别请参阅[支持的平台](../support.md)。

!!! Note "此前称为 `atuin hex`"

    `atuin pty-proxy` 取代了旧的 `atuin hex` 命令。出于向后兼容的考虑，`atuin hex` 仍可使用，但最终将被移除。

## TUI 渲染

搜索 TUI 面临一个取舍：界面要么以全屏替代屏幕（alt-screen）模式接管你的终端，要么以内联模式清除你之前的输出。两者都不理想。

使用 pty-proxy 时，Atuin 弹窗会渲染在你之前的输出之上，关闭后 pty-proxy 会完整恢复原有输出。

!!! tip "已经在用 tmux 了？"

    tmux 可以在不使用 pty-proxy 的情况下解决同样的问题：设置
    [`[tmux] enabled = true`](../configuration/config.md#tmux)，搜索界面就会以弹窗形式在你的窗格上方打开，而不会影响窗格本身。

## 捕获命令输出

由于 pty-proxy 位于你的终端与 shell 之间，它还可以记录每条命令打印的内容。它会读取你 shell 发出的 [OSC 133](https://gitlab.freedesktop.org/Per_Bothner/specifications/blob/master/proposals/prompts-data-model.md)
提示符标记，并借助这些标记判断一条命令的输出在何处结束、下一条命令从何处开始。随后，它会将每个捕获到的输出块交给
[daemon](daemon.md)，由 daemon 以命令的 Atuin 历史记录 ID 为键，将其保存在内存中。

正是这种捕获机制，让 AI 工具能够看到实际发生的情况，而不是仅凭命令本身去猜测：

- [Atuin AI](../ai/introduction.md) 可以通过其 `AtuinOutput` 工具读取真实的错误信息，从而回答"那条命令为什么会失败？"
- Claude Code 和 Cursor 等外部智能体，也可以通过 Atuin 的 [MCP 服务器](../ai/mcp.md)做到同样的事

输出捕获需要**同时**运行 pty-proxy 和 daemon。默认情况下不会捕获任何内容。设置方法、保留期限和隐私相关信息，请参阅[读取命令输出](../ai/command-output.md)。

## 初始化

Atuin pty-proxy 需要与你现有的 Atuin 配置分开初始化。请将下方所示的初始化行放入你 shell 的初始化脚本中，并尽量放在该脚本靠前的位置，即放在你常规的 `atuin init` 调用_之前_。

=== "zsh"

    ```shell
    eval "$(atuin pty-proxy init zsh)"
    ```

=== "bash"

    ```shell
    eval "$(atuin pty-proxy init bash)"
    ```

=== "fish"

    将

    ```shell
    atuin pty-proxy init fish | source
    ```

    添加到你 `~/.config/fish/config.fish` 文件中的 `is-interactive` 代码块内

=== "Nushell"

    在 *Nushell* 中运行：

    ```shell
    mkdir ~/.local/share/atuin/
    atuin pty-proxy init nu | save -f ~/.local/share/atuin/pty-proxy-init.nu
    ```

    添加到 `config.nu`，需放在常规的 `atuin init` **之前**：

    ```shell
    source ~/.local/share/atuin/pty-proxy-init.nu
    ```
    Nushell 的 `source` 命令要求使用静态文件路径，因此你必须预先生成该文件。

---

如果 `atuin` 二进制文件默认不在你的 `PATH` 中，你应当在设置好 `PATH` 后尽早初始化 pty-proxy。例如，对于将 Atuin 安装在 `~/.atuin/bin/atuin` 的 bash 用户，配置文件可能如下所示：

```bash
export PATH=$HOME/.atuin/bin:$PATH
eval "$(atuin pty-proxy init bash)"

# ... 其他 shell 配置 ...

eval "$(atuin init bash)"
```
