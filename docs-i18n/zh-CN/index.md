# Atuin

Atuin 用 SQLite 数据库取代你的 shell 历史记录，并为每条命令记录额外的上下文信息：命令运行时所在的目录、耗时、是否成功，以及来自哪台机器和哪个会话。正是这些上下文让搜索真正变得有用。

Atuin 还能将你的历史记录端到端加密后同步到你的所有机器上。你可以使用我们的服务器、[自托管服务器](self-hosting/server-setup.md)，或者完全跳过同步、只在本地使用。

## 快速入门 {#quickstart}

```bash
bash <(curl --proto '=https' --tlsv1.2 -sSf https://setup.atuin.sh)
```

重启你的 shell，然后按 ++ctrl+r++ 或 ++up++ 方向键进行搜索。输入查询内容后，按回车键运行选中的命令，或按 tab 键将其放到命令行中以便编辑。

导入你现有的历史记录：

```bash
atuin import auto
```

若要将历史记录同步到多台机器上——这是可选步骤，详见[设置同步](guide/sync.md)：

```bash
atuin register -u <USERNAME> -e <EMAIL>
atuin sync
```

如果你想一步一步来，[快速开始](guide/getting-started.md)会用更详细的说明带你走一遍相同的流程。之后，[基本用法](guide/basic-usage.md)介绍如何操作 TUI，[配置](configuration/config.md)则记录了每一个设置项。

## 支持的平台 {#supported-platforms}

Atuin 支持 zsh、bash、fish、nushell、xonsh 和 PowerShell。完整的支持矩阵以及各层级的含义，请参见[支持的平台](support.md)。

## 获取帮助 {#getting-help}

可以在[论坛](https://forum.atuin.sh)发帖讨论某个话题，加入我们的 [Discord](https://discord.gg/Fq8bJSKPHh)，或提交一个 [issue](https://github.com/atuinsh/atuin/issues)。如果遇到问题，[`atuin doctor`](reference/doctor.md) 会收集我们需要的详细信息。
