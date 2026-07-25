# history list

## `atuin history list`


| 参数              | 描述                                                                   |
|------------------|-------------------------------------------------------------------------------|
| `--cwd`/`-c`     | 仅列出当前目录的历史记录（默认：所有目录）               |
| `--session`/`-s` | 仅列出当前会话的历史记录（默认：false）                    |
| `--human`        | 以人类可读的格式显示时间戳和持续时间（默认：false） |
| `--cmd-only`     | 仅显示命令文本（默认：false）                            |
| `--reverse`      | 反转输出顺序（默认：false）                            |
| `--format`       | 自定义命令的输出格式（见下文）                             |
| `--print0`       | 使用空字符终止输出，以便更好地支持多行内容                                                                              |


## 格式

自定义 `history list` 的输出格式

示例

```shell
atuin history list --format "{time} - {duration} - {command}"
```

支持的变量

```text
{command}, {directory}, {duration}, {user}, {host} and {time}
```
