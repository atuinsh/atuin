# `atuin search`

```
atuin search <query>
```

Atuin 搜索还支持带有 `*` 或 `%` 字符的通配符。 默认情况下，会执行前缀搜索（即，所有查询都会自动附加通配符）。

| 参数               | 描述                                                  |
| ------------------ | ----------------------------------------------------- |
| `--cwd/-c`         | 列出历史记录的目录（默认：所有目录）                  |
| `--exclude-cwd`    | 不包括在此目录中运行的命令（默认值：none）            |
| `--exit/-e`        | 按退出代码过滤（默认：none）                          |
| `--exclude-exit`   | 不包括以该值退出的命令（默认值：none）                |
| `--before`         | 仅包括在此时间之前运行的命令（默认值：none）          |
| `--after`          | 仅包含在此时间之后运行的命令（默认值：none）          |
| `--interactive/-i` | 打开交互式搜索 UI（默认值：false）                    |
| `--human`          | 对时间戳和持续时间使用人类可读的格式（默认值：false） |

## 举例

```
# 打开交互式搜索 TUI
atuin search -i

# 打开预装了查询的交互式搜索 TUI
atuin search -i atuin

# 搜索所有以 cargo 开头且成功退出的命令。
atuin search --exit 0 cargo

# 从当前目录中搜索所有在2021年4月1日之前运行且失败的命令。
atuin search --exclude-exit 0 --before 01/04/2021 --cwd .

# 搜索所有以 cargo 开头，成功退出且是在昨天下午3点之后运行的命令。
atuin search --exit 0 --after "yesterday 3pm" cargo
```
