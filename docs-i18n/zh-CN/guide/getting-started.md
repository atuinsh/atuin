# 快速开始

本指南将逐步介绍如何正确设置 Atuin；如果你只想直接看命令，请参阅[主页上的快速入门](../index.md#quickstart)。

设置共分四步，只有第一步是必需的：

1. [安装 Atuin](installation.md) 及其 shell 插件
2. [导入已有的历史记录](import.md)
3. [设置同步](sync.md)（如果你想在多台机器上共享历史记录）
4. [了解 TUI](basic-usage.md)

## 1. 安装

安装脚本会处理好二进制文件和 shell 插件，并引导你完成其余步骤：

```shell
bash <(curl --proto '=https' --tlsv1.2 -sSf https://setup.atuin.sh)
```

然后重启你的 shell。想使用包管理器，或者想自己安装各个组件？请参阅[安装](installation.md)。

此时 Atuin 已经在记录新的命令了。按 ++ctrl+r++ 或 ++up++ 方向键即可搜索这些命令。

## 2. 导入已有的历史记录

Atuin 只会记录安装之后运行的命令，所以需要把旧的历史记录导入进来：

```shell
atuin import auto
```

这会检测你使用的 shell，并从其历史记录文件导入数据，该文件本身不会被修改——你的 shell 会照常继续向它写入内容。如需从特定 shell 或非默认文件导入，请参阅[导入已有历史记录](import.md)。

## 3. 设置同步（可选）

同步会备份你的历史记录，并在多台机器之间共享，全程端到端加密。你可以使用我们的服务器，也可以[自行搭建](../self-hosting/server-setup.md)。

```shell
atuin register -u <USERNAME> -e <EMAIL>
atuin sync
```

注册时会生成一个加密密钥。**请把它保存在安全的地方**——你需要用它在其他机器上登录，而且它一旦丢失将无法找回。详情请参阅[设置同步](sync.md)，包括如何在其他地方登录。

跳过这一步也没关系。你的历史记录会保留在这台机器上，不会被备份，也不会被同步。

## 4. 让它更懂你

一旦一切就绪：

- [基本用法](basic-usage.md)——Atuin 会记录什么，以及如何操作 TUI
- [过滤模式与搜索模式](advanced-usage.md)——将搜索范围缩小到当前目录、当前机器或当前会话
- [键位绑定](../configuration/key-binding.md)——包括如果你不喜欢 ++up++ 方向键的绑定，如何[禁用它](../configuration/key-binding.md#disable-up-arrow)
- [配置](../configuration/config.md)——所有设置项，包括用[内嵌窗口](../configuration/config.md#inline_height)代替全屏显示

## 获取帮助

可以在[论坛](https://forum.atuin.sh)发帖提问，加入我们的 [Discord](https://discord.gg/Fq8bJSKPHh)，或提交一个 [issue](https://github.com/atuinsh/atuin/issues)。运行 [`atuin doctor`](../reference/doctor.md) 可以收集我们需要的诊断信息。
