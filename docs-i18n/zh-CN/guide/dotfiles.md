# 同步 dotfiles

Atuin 最初是一个用于同步和搜索 shell 历史记录的工具，如今我们正在构建可以跨机器同步 dotfiles 的能力，让它们用起来更加省心。

目前，Atuin 支持管理和同步 shell 别名（alias）与环境变量，未来还会支持更多内容。

dotfiles 同步功能在 zsh、bash、fish、xonsh 和 PowerShell 上都可用。完整的支持矩阵请参阅[支持的平台](../support.md)。

注意：Atuin 会在内部处理你的配置，因此安装完成后，你无需再手动编辑配置文件。

## 必需的配置

设置并安装 Atuin 后，你需要在配置文件（`~/.config/atuin/config.toml`）中加入以下内容：

```toml
[dotfiles]
enabled = true
```

在后续版本中，这项功能将默认启用。

## 使用方法

### 别名（Aliases）

创建或删除别名后，记得重启 shell！

#### 创建别名

```shell
atuin dotfiles alias set NAME 'COMMAND'
```

例如，将 `k` 设为 `kubectl` 的别名：


```shell
atuin dotfiles alias set k 'kubectl'
```

或者将 `ll` 设为 `ls -lah` 的别名：

```shell
atuin dotfiles alias set ll 'ls -lah'
```

#### 删除别名

使用以下命令删除别名：

```shell
atuin dotfiles alias delete NAME
```

例如，删除上面创建的别名 `k`：

```shell
atuin dotfiles alias delete k
```

#### 列出别名

你可以使用以下命令列出所有别名：

```shell
atuin dotfiles alias list
```

### 环境变量（Env vars）

创建或删除环境变量后，记得重启 shell！

#### 创建变量

```shell
atuin dotfiles var set NAME 'value'
```

例如，将 `FOO` 设为 `bar`：


```shell
atuin dotfiles var set FOO 'bar'
```

变量默认会被导出（export），不过你也可以像下面这样创建一个仅限 shell 内部使用的变量：

```shell
atuin dotfiles var set -n foo 'bar'
```


#### 删除变量

使用以下命令删除变量：

```shell
atuin dotfiles var delete NAME
```

例如，删除上面创建的变量 `FOO`：

```shell
atuin dotfiles var delete FOO
```

#### 列出变量

你可以使用以下命令列出所有变量：

```shell
atuin dotfiles var list
```

### 同步与备份 dotfiles

如果你已经[设置好同步](sync.md)，那么运行

```shell
atuin sync
```

就会把你的配置备份到服务器，并在各台机器之间完成同步。
