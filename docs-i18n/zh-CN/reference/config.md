# config

## `atuin config`

读取、写入并检查 Atuin 的配置值。Atuin 会从多个来源解析配置（默认值、配置文件、环境变量）。使用 `config` 命令可以查看每个配置项的当前来源，无需打开编辑器即可修改 `config.toml` 中的值。

## 子命令

### `atuin config get <key>`

打印指定键在配置文件中的值。

```console
$ atuin config get search_mode
fuzzy

$ atuin config get daemon
[daemon]
enabled = true
socket_path = "/tmp/atuin_daemon.sock"
```

如果配置文件中不存在该键，你会看到：

```console
$ atuin config get enter_accept
(not set in config file)
```

#### `--resolved` / `-r`

打印合并默认值、配置文件与环境变量覆盖后得到的最终生效值：

```console
$ atuin config get enter_accept --resolved
false
```

该选项对表格类型的键同样适用，会以扁平化的点号分隔 key=value 形式展示所有已解析的子项：

```console
$ atuin config get logs --resolved
logs.ai.file = ai.log
logs.daemon.file = daemon.log
logs.dir = /home/user/.local/share/atuin/logs
logs.enabled = true
logs.level = info
logs.search.file = search.log
```

#### `--verbose` / `-v`

并排显示配置文件中的值与解析后的值：

```console
$ atuin config get enter_accept --verbose
Config file:
  (not set in config file)

Resolved:
  false
```

### `atuin config set <key> <value>`

在 `config.toml` 中设置配置值，文件原有的格式和注释都会被保留。

```shell
$ atuin config set search_mode fuzzy
$ atuin config set daemon.enabled true
```

#### 类型检测

默认情况下，如果该键已存在于配置文件中，`set` 会匹配现有值的 TOML 类型，从而避免像 `"300"` 这样的字符串被意外改写为整数 `300`。

对于新键（尚未存在于文件中的键），`set` 会自动检测其类型：

| 值            | 检测到的类型 |
|---------------|---------------|
| `true`/`false`| 布尔值       |
| `42`、`-1`    | 整数         |
| `3.14`        | 浮点数       |
| 其他任何值    | 字符串       |

!!! warning "仅支持标量值"
    `atuin config set` 只能设置标量值的配置项，如需修改表格或数组类型，请手动编辑配置文件。

#### `--type` / `-t`

使用显式类型覆盖自动检测：

```shell
$ atuin config set sync_frequency 600 --type string
```

可选值：`auto`、`string`、`boolean`、`integer`、`float`。

对表格类型的键执行 set 操作会报错，提示你改用点号分隔的键：

```console
$ atuin config set logs true
Error: 'logs' is a table; use a dotted key like 'logs.key' to set a value within it
```

### `atuin config print [key]`

以 TOML 格式打印配置文件中的值。不指定键时打印整个文件，指定键时打印对应部分：

```console
$ atuin config print daemon
[daemon]
enabled = true
socket_path = "/tmp/atuin_daemon.sock"
pidfile_path = "/tmp/atuin_daemon.pid"
autostart = false
```
