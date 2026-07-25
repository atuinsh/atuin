# 搜索

Atuin 的搜索支持使用 `*` 或 `%` 通配符。默认情况下会执行前缀搜索，也就是说，所有查询都会自动追加一个通配符。

| Arg                  | Description |
| -------------------- | ----------------------------------------------------------------------------- |
| `--cwd`/`-c`         | 列出该目录下的历史记录（默认：所有目录）                         |
| `--exclude-cwd`      | 排除在该目录下运行的命令（默认：无）             |
| `--exit`/`-e`        | 按退出代码过滤（默认：无）                                           |
| `--exclude-exit`     | 排除以该值退出的命令（默认：无）            |
| `--before`           | 只包含在此时间之前运行的命令（默认：无）                    |
| `--after`            | 只包含在此时间之后运行的命令（默认：无）                    |
| `--interactive`/`-i` | 打开交互式搜索界面（默认：false）                               |
| `--human`            | 对时间戳和持续时间使用易读格式（默认：false） |
| `--limit`            | 限制结果数量（默认：无）                                   |
| `--offset`           | 结果的起始偏移量（默认：无）                                    |
| `--delete`           | 删除匹配此查询的历史记录                                            |
| `--delete-it-all`    | 删除所有 shell 历史记录                                              |
| `--reverse`          | 反转搜索结果的顺序，最早的排在最前                                 |
| `--format`/`-f`      | 可用变量：{command}、{directory}、{duration}、{user}、{host}、{time}、{exit} 和 {relativetime}。示例：--format "{time} - [{duration}] - {directory}$\t{command}" |
| `--inline-height`    | 设置 Atuin 界面最多可占用的行数              |
| `--help`/`-h`        | 打印帮助信息                                                                   |

## `atuin search -i`

使用 Atuin 的交互式搜索 TUI 对你的历史记录进行模糊搜索。

![compact](https://user-images.githubusercontent.com/1710904/161623659-4fec047f-ea4b-471c-9581-861d2eb701a9.png)

你可以通过 `alt + #`（`#` 为想要重新执行的命令所在的行号）重新执行第 `nth` 条命令。

注意：目前 macOS 上还不支持此功能。

## 示例

```shell
# 打开交互式搜索 TUI
atuin search -i

# 打开交互式搜索 TUI，并预先加载一个查询
atuin search -i atuin

# 搜索所有以 cargo 开头且成功退出的命令
atuin search --exit 0 cargo

# 搜索所有在当前目录下运行、在 2021 年 4 月 1 日之前运行且失败的命令
atuin search --exclude-exit 0 --before 01/04/2021 --cwd .

# 搜索所有以 cargo 开头、成功退出，且在昨天下午 3 点之后运行的命令
atuin search --exit 0 --after "yesterday 3pm" cargo

# 删除所有以 cargo 开头、成功退出，且在昨天下午 3 点之后运行的命令
atuin search --delete --exit 0 --after "yesterday 3pm" cargo

# 搜索一个以 cargo 开头的命令，仅返回一条结果
atuin search --limit 1 cargo

# 搜索一个以 cargo 开头的命令的单条结果，跳过（偏移）一条结果
atuin search --offset 1 --limit 1 cargo

# 查找最早的 cargo 命令
atuin search --limit 1 --reverse cargo
```
