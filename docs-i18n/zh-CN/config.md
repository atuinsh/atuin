# 配置

Atuin 维护两个配置文件，存储在 `~/.config/atuin/` 中。 我们将数据存储在 `~/.local/share/atuin` 中（除非被 XDG\_\* 覆盖）。

您可以通过设置更改配置目录的路径 `ATUIN_CONFIG_DIR`。 例如

```
export ATUIN_CONFIG_DIR = /home/ellie/.atuin
```

## 客户端配置

```
~/.config/atuin/config.toml
```

客户端运行在用户的机器上，除非你运行的是服务器，否则这就是你所关心的。

见 [config.toml](../../atuin-client/config.toml) 中的例子

### `dialect`

这配置了 [stats](stats.md) 命令解析日期的方式。 它有两个可能的值

```
dialect = "uk"
```

或者

```
dialect = "us"
```

默认为 "us".

### `auto_sync`

配置登录时是否自动同步。默认为 true

```
auto_sync = true/false
```

### `sync_address`

同步的服务器地址！ 默认为 `https://api.atuin.sh`

```
sync_address = "https://api.atuin.sh"
```

### `sync_frequency`

多长时间与服务器自动同步一次。这可以用一种"人类可读"的格式给出。例如，`10s`，`20m`，`1h`，等等。默认为 `1h` 。

如果设置为 `0`，Atuin将在每个命令之后进行同步。一些服务器可能有潜在的速率限制，这不会造成任何问题。

```
sync_frequency = "1h"
```

### `db_path`

Atuin SQlite数据库的路径。默认为 
`~/.local/share/atuin/history.db`

```
db_path = "~/.history.db"
```

### `key_path`

Atuin加密密钥的路径。默认为 
`~/.local/share/atuin/key`

```
key = "~/.atuin-key"
```

### `session_path`

Atuin服务器会话文件的路径。默认为 
`~/.local/share/atuin/session` 。 这本质上只是一个API令牌

```
key = "~/.atuin-session"
```

### `search_mode`

使用哪种搜索模式。Atuin 支持 "prefix"（前缀）、"fulltext"（全文） 和 "fuzzy"（模糊）搜索模式。前缀(prefix)搜索语法为 "query\*"，全文(fulltext)搜索语法为 "\*query\*"，而模糊搜索适用的搜索语法 [如下所述](#fuzzy-search-syntax) 。

默认配置为 "fuzzy"

### `filter_mode`

搜索时要使用的默认过滤器

| 模式   | 描述	|
|--------------- | --------------- |
| global (default)   | 从所有主机、所有会话、所有目录中搜索历史记录  |
| host   | 仅从该主机搜索历史记录   |
| session   | 仅从当前会话中搜索历史记录   |
| directory | 仅从当前目录搜索历史记录|

过滤模式仍然可以通过 ctrl-r 来切换


```
search_mode = "fulltext"
```

#### `fuzzy` 的搜索语法

`fuzzy` 搜索语法的基础是 [fzf 搜索语法](https://github.com/junegunn/fzf#search-syntax) 。

| 内容     | 匹配类型                 | 描述                          |
| --------- | -------------------------- | ------------------------------------ |
| `sbtrkt`  | fuzzy-match                | 匹配 `sbtrkt` 的项目           |
| `'wild`   | exact-match (quoted)       | 包含 `wild` 的项目            |
| `^music`  | prefix-exact-match         | 以 `music` 开头的项目        |
| `.mp3$`   | suffix-exact-match         | 以 `.mp3` 结尾的项目           |
| `!fire`   | inverse-exact-match        | 不包括 `fire` 的项目     |
| `!^music` | inverse-prefix-exact-match | 不以 `music` 开头的项目 |
| `!.mp3$`  | inverse-suffix-exact-match | 不以 `.mp3` 结尾的项目    |


单个条形字符术语充当 OR 运算符。 例如，以下查询匹配以 `core` 开头并以 `go`、`rb` 或 `py` 结尾的条目。

```
^core go$ | rb$ | py$
```

## 服务端配置

`// TODO`
