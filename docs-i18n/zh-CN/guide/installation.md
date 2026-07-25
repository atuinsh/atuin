# 安装

## 推荐的安装方式

### 在 Unix 上

让我们开始吧！首先，你需要安装 Atuin。推荐使用安装脚本，它会自动完成 Atuin 的安装，包括处理你的环境所需的各项依赖。

该脚本会将二进制文件安装到 `~/.atuin/bin`；如果你想采用其他安装方式，下文的手动安装步骤能提供更大的灵活性。

```shell
curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh
```

安装脚本会引导你导入 shell 历史记录并设置同步账户。如果你想跳过这些交互式提示（例如在 CI 或 Dockerfile 中），可以传入 `--non-interactive`：

```shell
curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh -s -- --non-interactive
```

脚本还会自动检测非交互式环境（管道输入、无 TTY），并在这些情况下跳过提示。

[**设置同步**——继续下一步，或阅读下文以手动安装 Atuin。](sync.md)

### 在 Windows 上

在 Windows 上，推荐使用 WinGet 安装 Atuin。安装完成后，如果你使用 PowerShell，请将初始化命令添加到 PowerShell 配置文件中，并重启 shell。

```shell
winget install -e Atuinsh.Atuin
if (-not (Test-Path -Path $PROFILE)) { New-Item -ItemType File -Path $PROFILE -Force | Out-Null }
Write-Output 'atuin init powershell | Out-String | Invoke-Expression' >> $PROFILE
```

请注意，`$PROFILE` 路径可能因 PowerShell 版本而异。

[**设置同步**——继续下一步。](sync.md)

## 手动安装

### 安装二进制文件

如果你不想使用安装程序，可以按照以下步骤手动安装。

=== "Cargo"

    建议先使用 [rustup](https://rustup.rs/) 设置好 Rust
    工具链，然后运行：

    ```shell
    cargo install atuin --locked
    ```

=== "Homebrew"

    ```shell
    brew install atuin
    ```

=== "MacPorts"

    Atuin 也可以通过 [MacPorts](https://ports.macports.org/port/atuin/) 安装：

    ```shell
    sudo port install atuin
    ```

=== "mise"

    Atuin 也可以使用 [mise](https://github.com/jdx/mise) 安装

    ```shell
    mise use -g atuin@latest
    ```

=== "Nix"

    本仓库是一个 flake，可以使用 `nix profile` 安装：

    ```shell
    nix profile install "github:atuinsh/atuin"
    ```

    此外，Atuin 也已收录进 [nixpkgs](https://github.com/NixOS/nixpkgs)：

    ```shell
    nix-env -f '<nixpkgs>' -iA atuin
    ```

=== "Pacman"

    Atuin 可以通过 Arch Linux 的 [extra 仓库](https://archlinux.org/packages/extra/x86_64/atuin/) 安装：

    ```shell
    pacman -S atuin
    ```

=== "XBPS"

    Atuin 可以通过 Void Linux 的 [仓库](https://github.com/void-linux/void-packages/tree/master/srcpkgs/atuin) 安装：

    ```shell
    sudo xbps-install atuin
    ```

=== "Termux"

    Atuin 可以通过 Termux 包仓库安装：

    ```shell
    pkg install atuin
    ```

=== "zinit"

    Atuin 可以直接从 github-releases 安装：

    ```shell
    # 第 1 行：将 `atuin` 二进制文件作为命令，从 github release 中获取，只查找 .tar.gz 文件，使用解压后的归档中的 `atuin` 文件
    # 第 2 行：在 clone 时进行设置（创建 init.zsh、补全）
    # 第 3 行：pull 行为与 clone 相同，source init.zsh
    zinit ice as"command" from"gh-r" bpick"atuin-*.tar.gz" mv"atuin*/atuin -> atuin" \
        atclone"./atuin init zsh > init.zsh; ./atuin gen-completions --shell zsh > _atuin" \
        atpull"%atclone" src"init.zsh"
    zinit light atuinsh/atuin
    ```

=== "WinGet"

    Atuin 可以通过 WinGet 安装：

    ```shell
    winget install -e Atuinsh.Atuin
    ```

=== "Source"

    Atuin 基于最新的稳定版 Rust 构建，我们无法对旧版本的兼容性做出任何保证。建议使用 [rustup](https://rustup.rs/)。

    ```shell
    git clone https://github.com/atuinsh/atuin.git
    cd atuin
    cargo install --path crates/atuin --locked
    ```

!!! warning "请注意"

    如果你选择手动安装 Atuin，而不是使用推荐的安装脚本，
    仅安装二进制文件是不够的，你还应该设置 shell 插件。

---

### 安装 shell 插件

安装完二进制文件后，还需要安装 shell 插件。
如果你是通过安装脚本安装的，这些步骤应该都已经帮你完成了！
安装完成后，请记得重启 shell。

=== "zsh"

    ```shell
    echo 'eval "$(atuin init zsh)"' >> ~/.zshrc
    ```

    === "zinit"

        ```shell
        # 如果你_只_想安装 shell 插件，请这样做；否则请参考上文的“通过 zinit 安装一切”方案
        zinit load atuinsh/atuin
        ```

    === "Antigen"

        ```shell
        antigen bundle atuinsh/atuin@main
        ```

    === "Antidote"

        ```shell
        antidote install atuinsh/atuin
        ```

=== "bash"

    === "ble.sh"

        搭配 [ble.sh](https://github.com/akinomyoga/ble.sh) >= 0.4 使用时，Atuin 在 bash 中的表现最佳。

        在 `~/.bashrc` 中安装并加载 ble.sh（>= 0.4）之后，只需将 atuin 也添加到你的 `~/.bashrc` 中

        ```shell
        echo 'eval "$(atuin init bash)"' >> ~/.bashrc
        ```

    === "bash-preexec"

        你也可以使用 [bash-preexec](https://github.com/rcaloras/bash-preexec)，不过部分命令记录的持续时间和退出状态可能会有一些小问题。

        !!! warning "请注意"

            bash-preexec 目前存在[一个问题][bp-ignorespace]，会导致它不再
            遵循 `ignorespace`。虽然 Atuin 会忽略以空白字符开头的命令，但这些命令
            仍可能最终出现在你的 bash 历史记录中。请检查你的
            配置！其他所有 shell 都没有这个问题。

            要在 `bash < 4` 中搭配 bash-preexec 使用 `atuin < 18.10.0`，需要开启
            `enter_accept` 选项（该选项默认已开启）。最新版本的 Atuin
            （>= 18.10.0）没有此限制。

            bash-preexec 无法为子 shell 命令 `(...)`、函数定义 `func() { ...; }`、
            空的 for-in 语句 `for i in; do ...; done` 等正确调用 `preexec` 钩子，
            因此这些命令及其持续时间可能无法正确记录在 Atuin 的历史记录中。

        从 Atuin 18.18.0 开始，如果没有加载其他 preexec 后端（ble.sh 或外部
        的 bash-preexec 副本），`atuin init bash` 会自动加载 bash-preexec。要禁用
        此行为，请向 `atuin init` 传入 `ATUIN_NO_BUILTIN_PREEXEC=1`，例如：

        ```shell
        eval "$(ATUIN_NO_BUILTIN_PREEXEC=1 atuin init bash)"
        ```

        如果你愿意，也可以单独下载并安装 bash-preexec：

        ```shell
        curl https://raw.githubusercontent.com/rcaloras/bash-preexec/master/bash-preexec.sh -o ~/.bash-preexec.sh
        echo '[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh' >> ~/.bashrc
        ```

        然后设置 Atuin：

        ```shell
        echo 'eval "$(atuin init bash)"' >> ~/.bashrc
        ```

=== "fish"

    将

    ```shell
    atuin init fish | source
    ```

    添加到你的 `~/.config/fish/config.fish` 文件中的 `is-interactive` 代码块里

=== "Nushell"

    在 *Nushell* 中运行：

    ```shell
    mkdir ~/.local/share/atuin/
    atuin init nu | save ~/.local/share/atuin/init.nu
    ```

    添加到 `config.nu`：

    ```shell
    source ~/.local/share/atuin/init.nu
    ```

    ??? tip "可选：Atuin pty-proxy"
        pty-proxy 是一个轻量级的 pty 代理，它会将 Atuin 弹窗渲染在
        你先前的输出之上，关闭时再将其恢复——不清屏，也不
        全屏。要在 Nushell 中使用 pty-proxy，请先生成初始化脚本：

        ```shell
        mkdir ~/.local/share/atuin/
        atuin pty-proxy init nu | save -f ~/.local/share/atuin/pty-proxy-init.nu
        ```

        然后在你的 `config.nu` 中尽早 source 它，*在*
        常规的 atuin init 之前：

        ```shell
        source ~/.local/share/atuin/pty-proxy-init.nu
        source ~/.local/share/atuin/init.nu
        ```

        Nushell 的 `source` 命令要求使用静态文件路径，因此你必须
        预先生成这两个文件。

=== "xonsh"

    将
    ```shell
    execx($(atuin init xonsh))
    ```
    添加到你的 `~/.xonshrc` 文件末尾

=== "PowerShell"

    将以下内容添加到你的 `$PROFILE` 文件末尾：

    ```shell
    atuin init powershell | Out-String | Invoke-Expression
    ```

## 升级

运行 `atuin update`；如果该命令不可用，请重新运行安装脚本。

如果你是通过包管理器安装的 Atuin，也应该使用对应的包管理器来更新它。

## 卸载

如果你想卸载 Atuin，请查看[卸载页面](../uninstall.md)。

[bp-ignorespace]: https://github.com/rcaloras/bash-preexec/issues/115
