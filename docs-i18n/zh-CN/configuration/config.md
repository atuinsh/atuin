# 配置

Atuin 会在 `~/.config/atuin/` 中维护两个配置文件，并将数据存储在 `~/.local/share/atuin` 中（除非被 XDG\_\* 覆盖）。

配置文件的完整路径为 `~/.config/atuin/config.toml`。

配置文件的位置可以通过 ATUIN_CONFIG_DIR 覆盖。

### `db_path`

默认值：`~/.local/share/atuin/history.db`

Atuin SQLite 数据库的路径。

```toml
db_path = "~/.history.db"
```

### `key_path`

默认值：`~/.local/share/atuin/key`

Atuin 加密密钥的路径。

```toml
key_path = "~/.atuin-key"
```

### `session_path`

默认值：`~/.local/share/atuin/session`

Atuin 服务器会话文件的路径，本质上只是一个 API 令牌。

```toml
session_path = "~/.atuin-session"
```

### `dialect`

默认值：`us`

用于配置 [stats](../reference/stats.md) 命令解析日期的方式，可选两个值：

```toml
dialect = "uk"
```

或者

```toml
dialect = "us"
```

### `auto_sync`

默认值：`true`

配置在登录状态下是否自动同步。

```toml
auto_sync = true/false
```

### `update_check`

默认值：`true`

配置是否自动检查更新。

```toml
update_check = true/false
```

### `sync_address`

默认值：`https://api.atuin.sh`

要同步的服务器地址。

```toml
sync_address = "https://api.atuin.sh"
```

### `sync_frequency`

默认值：`1h`

自动与服务器同步的频率，可以使用「人类可读」的格式给出，例如 `10s`、`20m`、`1h` 等。

如果设置为 `0`，Atuin 将在每个命令之后进行同步。有些服务器可能会对频繁的同步进行限流，但这不会造成任何问题。

```toml
sync_frequency = "1h"
```

### `search_mode`

默认值：`fuzzy`

使用哪种搜索模式。Atuin 支持 `prefix`、`fulltext`、`fuzzy`、`daemon-fuzzy` 和 `skim` 搜索模式。

- `prefix` 模式搜索「query\*」。
- `fulltext` 模式搜索「\*query\*」。
- `fuzzy` 应用[模糊搜索语法](#fuzzy-search-syntax)。
- `skim` 应用 [skim 搜索语法](https://github.com/lotabout/skim#search-syntax)。

```toml
search_mode = "fuzzy"
```

!!! note "daemon-fuzzy 搜索模式"

    「daemon-fuzzy」模式是 Atuin 18.13 版本新增的。该搜索模式使用存储在守护进程中的内存索引，来执行快速且可自定义的搜索。

    要使用新的 `"daemon-fuzzy"` 模式，请启用守护进程，将 autostart 设置为 true（除非你自行管理其生命周期），并设置搜索模式：

    ```toml
    search_mode = "daemon-fuzzy"

    [daemon]
    enabled = true
    autostart = true
    ```

    你可以在此模式下自定义 frequency（频率）、recency（最近程度）和 frecency 分数各自的优先级，详见[分数乘数一节](#score-multipliers)。

#### `fuzzy` 搜索语法 {#fuzzy-search-syntax}

`fuzzy` 和 `daemon-fuzzy` 的搜索语法基于 [fzf 搜索语法](https://github.com/junegunn/fzf#search-syntax)。

| 内容      | 匹配类型                    | 描述                          |
| --------- | -------------------------- | ------------------------------------ |
| `sbtrkt`  | fuzzy-match                | 匹配 `sbtrkt` 的项目            |
| `'wild`   | exact-match (quoted)       | 包含 `wild` 的项目            |
| `^music`  | prefix-exact-match         | 以 `music` 开头的项目        |
| `.mp3$`   | suffix-exact-match         | 以 `.mp3` 结尾的项目           |
| `!fire`   | inverse-exact-match        | 不包含 `fire` 的项目     |
| `!^music` | inverse-prefix-exact-match | 不以 `music` 开头的项目 |
| `!.mp3$`  | inverse-suffix-exact-match | 不以 `.mp3` 结尾的项目    |

单个竖线字符作为 OR 运算符使用。例如，以下查询会匹配以 `core` 开头、并以 `go`、`rb` 或 `py` 结尾的条目。

```
^core go$ | rb$ | py$
```

!!! warning "daemon-fuzzy 不支持竖线操作符"
    「daemon-fuzzy」搜索模式目前不支持竖线字符运算符。

### `filter_mode`

默认值：`global`

交互式搜索启动时所使用的过滤模式。可接受的值为 `global`、`host`、`session`、`directory`、`workspace` 和 `session-preload`，每种模式具体搜索什么内容参见[过滤模式](../guide/advanced-usage.md#filter-mode)。

无论从哪种模式开始，你都可以通过 ctrl-r 循环切换其余模式。

```toml
filter_mode = "host"
```

### `search_mode_shell_up_key_binding`

默认值：`fuzzy`

从 shell 的上箭头键位绑定调用搜索时所使用的默认搜索模式。

接受与上面 `search_mode` 完全相同的选项。

```toml
search_mode_shell_up_key_binding = "fuzzy"
```

默认为 `search_mode` 所指定的值。

### `filter_mode_shell_up_key_binding`

默认值：`global`

从 shell 的上箭头键位绑定调用搜索时所使用的默认过滤器。

接受与上面 `filter_mode` 完全相同的选项。

```toml
filter_mode_shell_up_key_binding = "session"
```

默认为 `filter_mode` 所指定的值。

### `inline_height_shell_up_key_binding`

当从 shell 的上箭头键位绑定调用 `atuin` 时，界面应占用的最大行数。

可接受的值与 `inline_height` 相同。

未设置时，将使用 `inline_height` 的值。

```toml
inline_height_shell_up_key_binding = 10
```

### `workspaces`

默认值：`false`

此标志启用一个名为「workspace」的伪过滤模式：当你处于 git 仓库中时，该过滤器会自动激活。

启用 workspace 过滤后，Atuin 会筛选在 git 仓库树内任意目录中执行过的命令。

过滤模式仍然可以通过 ctrl-r 切换。

```toml
workspaces = false
```

### `style`

默认值：`compact`

使用哪种样式。可选值：`auto`、`full` 和 `compact`。

- `compact`：

![compact](https://user-images.githubusercontent.com/1710904/161623659-4fec047f-ea4b-471c-9581-861d2eb701a9.png)

- `full`：

![full](https://user-images.githubusercontent.com/1710904/161623547-42afbfa7-a3ef-4820-bacd-fcaf1e324969.png)

使用 `auto` 时，Atuin 默认使用 `full` 模式，但当终端窗口过短、无法正常显示 `full` 模式时，会自动切换到 `compact` 模式。

```toml
style = "compact"
```

### `invert`

默认值：`false`

反转 UI，将搜索栏放在顶部。

```toml
invert = true/false
```

### `inline_height`

默认值：`40`

设置 Atuin 界面应占用的最大行数。

如果设置为 `0`，Atuin 将始终占用所有可用行数（全屏）。

```toml
inline_height = 40
```

### `show_preview`

默认值：`true`

配置是否显示所选命令的预览。

当命令长度超过终端宽度并被截断时，这个选项很有用。

```toml
show_preview = true
```

### `max_preview_height`

默认值：`4`

配置预览显示的最大高度。

当历史记录中有较长的脚本，而你希望通过前几行以上的内容来区分它们时，这个选项很有用。

```toml
max_preview_height = 4
```

### `show_help`

默认值：`true`

配置是否显示帮助行，其中包含当前 Atuin 版本（以及是否有可用更新）、键位映射提示，以及历史记录中的命令总数。

```toml
show_help = true
```

### `show_tabs`

默认值：`true`

配置是否显示用于搜索和检查的标签页。

```toml
show_tabs = true
```

### `auto_hide_height`

默认值：`8`

当可用高度低于该行数时，隐藏多余的 UI 行。此设置仅在使用 `compact` 样式时才生效（参见上面的 `style`），且目前仅适用于交互式搜索和检查器。将其设置为 `0` 可完全关闭此功能。

```toml
auto_hide_height = 8
```

### `exit_mode`

默认值：`return-original`

搜索时按下 Esc 键该执行什么操作。

| 值                        | 行为                                                        |
| ------------------------- | ---------------------------------------------------------- |
| return-original（默认）   | 将命令行设置为开始搜索前的值                                |
| return-query               | 将命令行设置为你目前为止输入的搜索查询                     |

按下 ctrl+c 或 ctrl+d 将始终返回原始的命令行值。

```toml
exit_mode = "return-query"
```

### `history_format`

`history list` 使用的默认格式。也可以在每次调用时通过 `--format` 参数指定，该参数的优先级高于此配置值。

更多内容参见 [history list](../reference/list.md)。

```toml
history_format = "{time}\t{command}\t{duration}"
```

### `history_filter`

使用 `history_filter` 将命令排除在历史记录跟踪之外——也许你想让所有的 `curl` 命令完全不出现在你的 shell 历史记录中，也可能只想排除匹配某种模式的一部分命令。

它支持正则表达式，因此你几乎可以隐藏任何你想要的内容！

```toml
## 请注意，这些正则表达式是未锚定的，也就是说，如果它们不是以 ^ 开头
## 或以 $ 结尾，则会匹配命令中的任意位置。
history_filter = [
   "^secret-cmd",
   "^innocuous-cmd .*--secret=.+"
]
```

### `cwd_filter`

使用 `cwd` 过滤器将目录排除在历史记录跟踪之外。

它支持正则表达式，因此你几乎可以隐藏任何你想要的内容！

```toml
## 请注意，这些正则表达式是未锚定的，也就是说，如果它们不是以 ^ 开头
## 或以 $ 结尾，则会匹配路径中的任意位置。
# cwd_filter = [
#   "^/very/secret/directory",
# ]
```

更新该参数后，你可以运行[prune 命令](../reference/prune.md)来删除与新过滤器匹配的旧历史记录条目。

### `store_failed`

默认值：`true`

```toml
store_failed = true
```

配置是否存储执行失败的命令（即退出状态非零的命令）。

### `secrets_filter`

默认值：`true`

```toml
secrets_filter = true
```

将每条命令与一组内置的正则表达式进行匹配，如果匹配到任意一条，则拒绝保存该命令。目前覆盖的模式包括：

| 服务 | 匹配内容 |
|---------|---------|
| AWS | Access key ID，以及设置 `AWS_SECRET_ACCESS_KEY` 或 `AWS_SESSION_TOKEN` 的命令 |
| Azure | 设置 `AZURE_*_KEY` 的命令 |
| Google Cloud | 设置 `GOOGLE_SERVICE_ACCOUNT_KEY` 的命令 |
| GitHub | 个人访问令牌（新旧两种）、OAuth 访问令牌（应用和用户）、应用安装令牌，以及刷新令牌 |
| GitLab | 个人访问令牌 |
| Slack | OAuth v2 机器人令牌和用户令牌，以及 webhook URL |
| Stripe | 生产密钥和测试密钥 |
| Netlify | 身份验证令牌 |
| npm | 令牌 |
| Pulumi | 个人访问令牌 |
| Atuin | `atuin login`，它将你的密码和加密密钥作为参数 |

有关具体的表达式，参见 [`secrets.rs`](https://github.com/atuinsh/atuin/blob/main/crates/atuin-client/src/secrets.rs)。

!!! note

    这是一层安全网，而非绝对保证。它只能捕获已识别格式的凭据——对于其他你需要排除的内容，请使用
    [`history_filter`](#history_filter)，并参见
    [从历史记录中排除命令](../guide/excluding-commands.md)。

### macOS 上的 Ctrl-n 键快捷方式

默认值：`true`

macOS 没有 ++alt++ 键，不过终端模拟器通常可以配置为将 ++option++ 键映射为 ++alt++ 使用。*但是*，以这种方式重新映射 ++option++ 可能会导致无法输入某些字符，例如在英式英语键盘布局下使用 ++option+3++ 输入 `#`。对于这种情况，请在配置文件中将 `ctrl_n_shortcuts` 选项设置为 `true`，从而将 ++alt+0++ 到 ++alt+9++ 的快捷键替换为 ++ctrl+0++ 到 ++ctrl+9++：

```toml
# 使用 Ctrl-0 .. Ctrl-9 代替 Alt-0 .. Alt-9 UI 快捷键
ctrl_n_shortcuts = true
```

### `show_numeric_shortcuts`

默认值：`true`

是否在 TUI 的列表项旁边显示数字快捷键（1..9）。如果你觉得这些不断变化的数字容易让人分心，可以将其设置为 `false` 来隐藏。

```toml
show_numeric_shortcuts = true
```

### `network_timeout`

默认值：`30`

等待网络请求的最长时间（以秒为单位）。如果与同步服务器之间的任何操作耗时超过此值，代码将直接失败，而不是无限期等待。

```toml
network_timeout = 30
```

### `network_connect_timeout`

默认值：`5`

Atuin 等待与远程同步服务器建立连接的最长时间（以秒为单位）。超过此时间，请求将失败。

```toml
network_connect_timeout = 5
```

### `extra_headers`

默认值：`{}`

在每次向同步服务器发出的请求中发送的额外 HTTP 头。当自托管服务器位于需要自身身份验证头的代理或访问网关（例如 Cloudflare Access）之后时，这非常有用。

Atuin 自身设置的请求头（例如 `Authorization`）无法被覆盖，因为 Atuin 的值始终优先。

为避免泄露凭据，当配置了额外的请求头时，Atuin 会拒绝跟随跨域重定向——这些请求头永远不会发送到你所配置的源站之外的其他源站。

```toml
extra_headers = { "CF-Access-Client-Id" = "...", "CF-Access-Client-Secret" = "..." }
```

### `local_timeout`

默认值：`5`

获取本地数据库连接（SQLite）的超时时间（以秒为单位）。

```toml
local_timeout = 5
```

### `command_chaining`

默认值：`false`

使用此选项可通过 `&&` 或 `||` 运算符构建命令链。启用后，打开 Atuin 时会搜索链中的下一条命令，并将其追加到当前输入缓冲区中。

```toml
command_chaining = false
```

### `enter_accept`

默认值：`false`

设置为 true 时，Atuin 将默认立即执行命令，而不需要用户按两次回车。按 Tab 键会返回 shell，让用户有机会先编辑命令。

严格来说，这项设置对新用户默认是 true，对已有用户默认是 false。我们已经在默认配置文件中将 `enter_accept` 设置为 `true`。在以后的版本中，这很可能会成为所有人的默认值。

```toml
enter_accept = false
```

### `keymap_mode`

默认值：`emacs`

交互式 Atuin 搜索（例如由 shell 中的键位绑定启动）的初始键位映射模式。支持四个值：`"emacs"`、`"vim-normal"`、`"vim-insert"` 和 `"auto"`。键位映射模式 `"emacs"` 是最基本的一种。在 `"vim-normal"` 键位映射模式下，你可以像在 Vim 中一样使用 ++k++
和 ++j++ 在历史记录列表中导航，而按下

++i++ 会将键位映射模式切换为 `"vim-insert"`。在 `"vim-insert"` 键位映射模式下，你可以像在 `"emacs"` 键位映射模式下一样搜索字符串，同时按下 ++esc++
会将键位映射模式切换回 `"vim-normal"`。设置为 `"auto"` 时，初始键位映射模式会根据触发
Atuin 搜索的 shell 键位映射自动确定。目前 Nushell 尚不支持 `"auto"`，在 Nushell 中始终会以
`"emacs"` 键位映射模式触发 Atuin 搜索。

```toml
keymap_mode = "emacs"
```

### `keymap_cursor`

默认值：`（空字典）`

Atuin 搜索中每种键位映射模式所对应的终端光标样式，通过一个字典指定，其键和值分别为键位映射名称和光标样式。键为 `emacs`、`vim_insert`、`vim_normal` 三者之一，值为光标样式之一，`default` 或
`{blink,steady}-{block,underline,bar}`。示例如下：

```toml
keymap_cursor = { emacs = "blink-block", vim_insert = "blink-block", vim_normal = "steady-block" }
```

如果指定了光标样式，那么当 Atuin 搜索以对应的键位映射模式启动、或切换到该模式时，终端的光标样式会被更改为指定的样式。此外，Atuin 搜索结束时，终端的光标样式会被重置为与 shell 键位映射所对应的键位映射模式相关联的样式。

### `prefers_reduced_motion`

默认值：`false`

启用此选项后，Atuin 将尽可能减少 TUI 中的动效。对动效敏感的用户可能会觉得实时更新的时间戳令人分心。

或者，也可以设置环境变量 NO_MOTION。

```toml
prefers_reduced_motion = false
```

## search

### `filters`

交互式搜索中可用的过滤模式列表，按你按下 ctrl-r 时循环切换的顺序排列。默认情况下所有模式都是启用的。从此列表中移除某个模式会将其完全禁用。当不在 git 仓库中，或 `workspaces = false` 时，会跳过 `workspace` 模式。有关每种模式的说明，参见[过滤模式](../guide/advanced-usage.md#filter-mode)。

`filter_mode` 设置会从此列表中选择初始模式。如果 `filter_mode` 被设置为列表中不存在的模式，则会改用第一个可用模式。

```toml
[search]
filters = ["global", "host", "session", "directory"]
```

### 分数乘数 {#score-multipliers}

对于[`"daemon-fuzzy"` 搜索模式](#search_mode)，你可以控制匹配项的评分方式。系统根据三个数值对匹配项打分：frequency、recency 和 frecency：

* Frequency（频率）——该完全匹配项被运行过多少次，且存在边际递减效应
* Recency（最近程度）——该完全匹配项上一次运行距今有多久
* Frecency——frequency 与 recency 的综合体现

frecency 的计算方式为 `Recency Score * Recency Multiplier + Frequency Score * Frequency Multiplier`。通过修改下面的选项，你可以自定义评分计算中各部分的相对重要程度。

对于每个设置项，值为 `1.0`（默认值）表示该分数将按原样使用。小于 `1.0` 的值会降低该分数的影响，大于 `1.0` 的值会增加该分数的影响。

例如，如果你非常在意某条命令的运行频率、而不太在意它最近是否运行过，可以将 `frequency_score_multiplier` 设置为 `10.0`，将 `recency_score_multiplier` 设置为 `0.1`。

!!! warning "仅适用于 daemon-fuzzy 模式"
    此处展示的分数乘数设置仅在 `"daemon-fuzzy"` 搜索模式下生效。

#### `frequency_score_multiplier`

默认值：`1.0`

在 frecency 计算中应用于 frequency 分数的乘数。将其设置为 `0` 会完全禁用 frecency 评分中的 frequency 部分。

```toml
frequency_score_multiplier = 1.0
```

#### `recency_score_multiplier`

默认值：`1.0`

在 frecency 计算中应用于 recency 分数的乘数。将其设置为 `0` 会完全禁用 frecency 评分中的 recency 部分。

```toml
recency_score_multiplier = 1.0
```

#### `frecency_score_multiplier`

默认值：`1.0`

用于最终 frecency 分数的乘数。将其设置为 `0` 会完全禁用 frecency 评分，仅依赖模糊匹配器自身的分数。

```toml
frecency_score_multiplier = 1.0
```

示例：

```toml
search_mode = "daemon-fuzzy"

[daemon]
enabled = true
autostart = true

[search]
recency_score_multiplier = 10.0
frequency_score_multiplier = 0.8
frecency_score_multiplier = 2.0
```

### 按作者过滤

交互式搜索只会显示你自己运行的命令，隐藏那些通过[代理钩子](../guide/agent-hooks.md)由 AI 编程代理记录的命令。目前这一行为无法在 `config.toml` 中配置。

要在命令行中按作者过滤，请使用 `atuin search --author`。可用的值参见
[按作者过滤](../guide/agent-hooks.md#filtering-by-author)。

#### `shells`

Atuin 版本：>= 18.18

默认值：`"auto"`

根据运行每条命令所使用的 shell，过滤交互式搜索结果。

| 值  | 含义 |
|--------|---------|
| `"all"` | 显示所有 shell 的命令。 |
| `"auto"` | 显示当前 shell 的命令，或未记录 shell 信息的命令（例如来自旧版本 Atuin 的记录）。 |
| 字符串数组 | 显示由数组中任意 shell 运行的命令。`""` 表示包含未记录 shell 信息的命令。 |

当前 shell 是根据 `ATUIN_SHELL` 环境变量检测的（由 shell 初始化脚本设置）。

```toml
[search]
# 默认值：显示当前 shell 的命令。从 Bash 调用时，Atuin 会显示 Bash 命令，
# 从 Zsh 调用时会显示 Zsh 命令，以此类推。同时也包含未记录 shell 信息的
# 命令（很可能来自旧版本的 Atuin）。
shells = "auto"

# 显示所有 shell 的命令。
# shells = "all"

# 仅显示 Bash 和 Zsh 的命令。
# shells = ["bash", "zsh"]
```

## 统计数据

客户端配置的这一部分专门用于配置 Atuin 统计数据的计算方式。

```toml
[stats]
common_subcommands = [...]
common_prefix = [...]
```

### `common_subcommands`

默认值：

```toml
common_subcommands = [
  "apt",
  "cargo",
  "composer",
  "dnf",
  "docker",
  "git",
  "go",
  "ip",
  "jj",
  "kubectl",
  "nix",
  "nmcli",
  "npm",
  "pecl",
  "pnpm",
  "podman",
  "port",
  "systemctl",
  "tmux",
  "yarn",
]
```

配置哪些命令应将其子命令视为统计数据的一部分。例如，统计时使用 `kubectl get` 而不是仅仅 `kubectl`。

### `common_prefix`

默认值：

```toml
common_prefix = [
  "sudo",
]
```

配置应从统计数据计算中完全剔除的命令。例如，应忽略「sudo」。

## `dotfiles`

默认值：`false`

启用主机之间 shell 别名的同步。

在你使用 Atuin 的每台机器上，将新的部分添加到配置文件的末尾：

```toml
[dotfiles]
enabled = true
```

使用命令行选项管理别名：

```shell
# 将 'k' 设为 'kubectl' 的别名
atuin dotfiles alias set k kubectl

# 列出所有别名
atuin dotfiles alias list

# 删除一个别名
atuin dotfiles alias delete k
```

设置别名后，你需要重启 shell 或重新 source 初始化文件，更改才会生效。

## keys

客户端配置的这一部分专门用于配置与按键相关的设置。

```toml
[keys]
scroll_exits = [...]
prefix = 'a'
```

### `scroll_exits`

默认值：`true`

配置当滚动超过最后一条或第一条记录时，TUI 是否退出。

```toml
scroll_exits = true
```

### `prefix`

默认值：`a`

使用哪个键作为前缀键。前缀模式是一套两步式的快捷键系统：先按 ++ctrl++ 和前缀键进入前缀模式，再按第二个键触发相应操作。例如，使用默认前缀 `a` 时，先按 ++ctrl+a++ 再按 ++d++ 会删除所选条目。

有关默认前缀快捷键的完整列表，参见[键位绑定页面](key-binding.md#prefix-mode)；如需自定义，参见[高级键位绑定页面](advanced-key-binding.md#custom-prefix-bindings)。

```toml
prefix = "a"
```

### `exit_past_line_start`

默认值：`true`

当光标位于行首时向左滚动，将退出 TUI。

```toml
exit_past_line_start = true
```

### `accept_past_line_end`

默认值：`true`

右箭头键的功能与 Tab 相同，会将所选行复制到命令行以供修改。

```toml
accept_past_line_end = true
```

### `accept_past_line_start`

默认值：`false`

左箭头键的功能与 Tab 相同，会将所选行复制到命令行以供修改。

```toml
accept_past_line_start = false
```

### `accept_with_backspace`

默认值：`false`

退格键的功能与 Tab 相同，会将所选行复制到命令行以供修改。

```toml
accept_with_backspace = false
```

## preview

客户端配置的这一部分专门用于配置与预览相关的设置。
（未来另外两个预览设置也会移到这里。）

```toml
[preview]
strategy = [...]
```

### `strategy`

默认值：`auto`

使用哪种预览策略来计算预览高度，它会遵循 `max_preview_height`。

| 值             | 预览高度根据……的长度计算 |
| -------------- | --------------------------------------------------- |
| auto（默认） | 所选命令                                    |
| static         | 当前结果集中最长的命令           |
| fixed          | 使用 `max_preview_height` 作为固定值             |

使用 `auto` 时，如果命令长度超过终端宽度，则会显示预览。

```toml
strategy = "auto"
```

## tmux

当你在 tmux 中时，搜索 UI 会在一个悬浮于当前面板之上的
[弹出窗口](https://github.com/tmux/tmux/wiki/Getting-Started#popups) 中打开，而不是直接绘制在面板上。弹出窗口会在你当前的工作目录中打开，并在你接受某条命令或退出时关闭。

```toml
[tmux]
enabled = true
width = "80%"
height = "60%"
```

只要弹出窗口无法使用——例如不在 tmux 中、tmux 版本低于 3.2，或所处的 shell 不支持它——Atuin 就会回退到正常的渲染方式，且不会报错。

!!! note "要求"

    - tmux >= 3.2，`display-popup` 从该版本起具备了 Atuin 所需的行为
    - zsh、bash 或 fish —— nushell、xonsh 和 PowerShell 目前还不支持该弹出窗口

这些设置由 `atuin init` 读取，并通过环境变量传递给 shell 插件，因此**更改后请重启 shell**。如果只想为单次会话禁用弹出窗口而不修改配置，请在 Atuin 的键位绑定运行之前设置
`ATUIN_TMUX_POPUP=false`。

### `enabled`

默认值：`false`

是否在 tmux 弹出窗口中显示搜索 UI。

```toml
enabled = true
```

### `width`

默认值：`"80%"`

弹出窗口的宽度，会传递给 `tmux display-popup -w`。可接受终端宽度的百分比，或绝对列数。

```toml
width = "80%"
```

### `height`

默认值：`"60%"`

弹出窗口的高度，会传递给 `tmux display-popup -h`。可接受终端高度的百分比，或绝对行数。

```toml
height = "60%"
```

## 守护进程

### enabled

默认值：`false`

启用后台守护进程。

将新的部分添加到配置文件的末尾：

```toml
[daemon]
enabled = true
```

### autostart

默认值：`false`

在需要时自动启动并管理守护进程。此选项与 `systemd_socket = true` 不兼容。如果已经在运行旧版实验性守护进程，请在使用 autostart 之前先手动重启一次。

```toml
autostart = false
```

### `sync_frequency`

默认值：`300`

守护进程同步的频率（以秒为单位）。

```toml
sync_frequency = 300
```

### `socket_path`

默认值：

```toml
socket_path = "~/.local/share/atuin/atuin.sock"
```

用于客户端到守护进程通信的 Unix 套接字绑定位置。

如果 XDG_RUNTIME_DIR 可用，Atuin 会改用该目录。

### `pidfile_path`

默认值：

```toml
pidfile_path = "~/.local/share/atuin/atuin-daemon.pid"
```

用于进程协调的守护进程 `pidfile` 路径。

### `systemd_socket`

默认值：`false`

使用通过 systemd 套接字激活协议传递的套接字，而不是使用路径。

```toml
systemd_socket = false
```

### `tcp_port`

默认值：`8889`

用于客户端到守护进程通信的端口。仅在非 Unix 系统上使用。

```toml
tcp_port = 8889
```

## logs

日志文件的行为。

```toml
[logs]
enabled = true
dir = "~/.atuin/logs"
level = "info"
retention = 4
```

### enabled

默认值：`true`

是否启用基于文件的日志记录。

```toml
enabled = true
```

### dir

默认值：`"~/.atuin/logs"`

存储日志文件的目录。

```toml
dir = "~/.atuin/logs"
```

### level

默认值：`"info"`

使用的日志级别。有效值为 `"trace"`、`"debug"`、`"info"`、`"warn"` 和 `"error"`，按详细程度从高到低排列。

```toml
level = "info"
```

### retention

默认值：`4`

保留日志文件的天数（按文件类型分别计算）。超过此天数的文件将被删除。

```toml
retention = 4
```

### ai

一个子对象，包含 AI 日志的特定选项：

* `enabled`——是否输出 AI 日志；默认为 `logs.enabled`
* `file`——AI 日志使用的文件名；默认为 `"ai.log"`。始终相对于 `logs.dir`。
* `level`——覆盖 AI 日志的日志级别；默认为 `logs.level`
* `retention`——保存 AI 日志的天数；默认为 `logs.retention`

```toml
[logs.ai]
enabled = true
file = "ai.log"
level = "info"
retention = 4
```

### daemon

一个子对象，包含守护进程日志的特定选项：

* `enabled`——是否输出守护进程日志；默认为 `logs.enabled`
* `file`——守护进程日志使用的文件名；默认为 `"daemon.log"`。始终相对于 `logs.dir`。
* `level`——覆盖守护进程日志的日志级别；默认为 `logs.level`
* `retention`——保存守护进程日志的天数；默认为 `logs.retention`

```toml
[logs.daemon]
enabled = true
file = "daemon.log"
level = "info"
retention = 4
```

### search

一个子对象，包含搜索日志的特定选项：

* `enabled`——是否输出搜索日志；默认为 `logs.enabled`
* `file`——搜索日志使用的文件名；默认为 `"search.log"`。始终相对于 `logs.dir`。
* `level`——覆盖搜索日志的日志级别；默认为 `logs.level`
* `retention`——保存搜索日志的天数；默认为 `logs.retention`

```toml
[logs.search]
enabled = true
file = "search.log"
level = "info"
retention = 4
```

## theme

用于显示终端界面的主题。

```toml
[theme]
name = "default"
debug = false
max_depth = 10
```

### `name`

默认值：`"default"`

主题名称，必须是内置主题之一（未设置或 `default` 表示默认主题，此外还有 `autumn` 或 `marine`），或者是主题目录中以 `.toml` 为后缀的文件。该目录默认为 `~/.config/atuin/themes/`，但可以通过 `ATUIN_THEME_DIR` 环境变量覆盖。

```toml
name = "my-theme"
```

### `debug`

默认值：`false`

输出主题无法加载的原因信息。此选项独立于其他日志级别，因为它可能导致主题文件中的数据未经过滤地打印到终端。

```toml
debug = false
```

### `max_depth`

默认值：10

遍历主题「父级关系」的层数。在正常使用中通常不需要添加或更改此项。

```toml
max_depth = 10
```

## `ui`

配置交互式搜索 UI 的外观。

```toml
[ui]
columns = ["duration", "time", "command"]
```

### `columns`

默认值：`["duration", "time", "command"]`

交互式搜索中从左到右显示的列。选中指示符（`" > "`）始终会隐式地显示在最前面。

每一列都可以指定为：
- 一个纯字符串（使用默认宽度）：`"duration"`
- 一个包含类型及可选宽度/expand 的对象：`{ type = "directory", width = 30 }`

#### 可用的列类型

| 列          | 默认宽度 | 描述                                     |
| ----------- | -------------- | ----------------------------------------------- |
| `duration`  | 5              | 命令执行耗时（例如 "123ms"）      |
| `time`      | 8              | 距执行的相对时间（例如 "59m ago"） |
| `datetime`  | 16             | 绝对时间戳（例如 "2025-01-22 14:35"）   |
| `directory` | 20             | 工作目录（过长时会被截断）       |
| `host`      | 15             | 运行命令时所在的主机名                  |
| `user`      | 10             | 用户名                                        |
| `exit`      | 3              | 退出代码（按成功/失败着色）          |
| `command`   | *              | 命令本身（默认会自动扩展）         |

#### 列选项

- **type**：列类型（使用对象格式时必填）
- **width**：以字符为单位的自定义宽度（可选，未指定时使用默认值）
- **expand**：如果为 `true`，该列会填满剩余空间。`command` 列默认为 `true`，其他列默认为 `false`。应当只有一列设置 `expand = true`。

#### 示例

```toml
# 精简模式——为命令留出更多空间
columns = ["duration", "command"]

# 自定义目录列宽度
columns = ["duration", { type = "directory", width = 30 }, "command"]

# 为多机同步用户显示主机名
columns = ["duration", "time", "host", "command"]

# 醒目地显示退出代码
columns = ["exit", "duration", "command"]

# 让目录列扩展，而不是命令列
columns = ["duration", "time", { type = "directory", expand = true }, { type = "command", expand = false }]
```

### `syntax_highlight`

默认值：`true`

对搜索结果中的命令进行语法高亮，使用运行该命令的 shell 所对应的语法进行解析：bash/zsh/sh 使用 bash 语法，fish 使用 fish 语法，而没有对应语法的 shell（nu、xonsh、PowerShell）则不高亮显示。选中行仍保持通常的单一高亮颜色。

默认颜色为 ANSI 调色板颜色，因此会自动匹配你终端的配色方案，也可以通过[主题](../guide/theming.md)中的 `Syntax*` 键进行自定义。

在 tree-sitter 无法构建的平台（例如 Windows）上不可用，因此在这些平台上命令会以未高亮的形式显示。

```toml
syntax_highlight = false
```

## ai

Atuin AI 的设置列在[单独的一节](../ai/settings.md)中。
