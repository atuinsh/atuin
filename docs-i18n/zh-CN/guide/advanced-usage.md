# 高级用法

有两个设置决定了你每次搜索的行为：**过滤模式**决定 Atuin 搜索*哪些*命令，**搜索模式**决定 Atuin *如何*用你的查询匹配这些命令。两者都可以在 TUI 中随时切换。

## 过滤模式

过滤模式会缩小 Atuin 搜索的历史记录范围。在 TUI 中按 **ctrl-r** 可在各模式间切换。

| 模式                | 搜索范围                                                                             |
|------------------|--------------------------------------------------------------------------------------|
| global（默认）        | 你在所有机器上的完整历史记录                                                          |
| host             | 仅限本机的历史记录                                                                     |
| session          | 仅限当前 shell 会话的历史记录                                                          |
| directory        | 仅限当前目录的历史记录                                                                 |
| workspace        | 仅限当前 git 仓库中任意位置的历史记录                                                  |
| session-preload  | 当前会话，加上会话开始之前的全部全局历史记录                                            |

`workspace` 模式需要开启 [`workspaces = true`](../configuration/config.md#workspaces)。当你不在 git 仓库中时，Atuin 会跳过该模式。

要更改搜索启动时所使用的模式，请设置 [`filter_mode`](../configuration/config.md#filter_mode)。要将某些模式完全从 ctrl-r 的轮换列表中移除，请设置 [`search.filters`](../configuration/config.md#filters)。
上箭头键可以使用与 ctrl-r 不同的起始模式——参见 [`filter_mode_shell_up_key_binding`](../configuration/config.md#filter_mode_shell_up_key_binding)。

## 搜索模式

搜索模式决定 Atuin 如何解析你的查询文本。在 TUI 中按 **ctrl-s** 可在各模式间切换。

| 模式             | 匹配方式                                                                                                        |
|-----------------|------------------------------------------------------------------------------------------------------------------|
| fuzzy（默认）      | 模糊匹配，使用 [fzf 语法](https://github.com/junegunn/fzf#search-syntax)——参见 [模糊搜索语法](../configuration/config.md#fuzzy-search-syntax) |
| prefix          | 匹配以你的查询开头的命令                                                                                            |
| fulltext        | 匹配在任意位置包含你的查询的命令                                                                                     |
| skim            | 使用 [skim 语法](https://github.com/lotabout/skim#search-syntax)                                                    |
| daemon-fuzzy    | 与 `fuzzy` 类似，但由[守护进程](../reference/daemon.md)的内存索引提供服务，评分可调                                    |

要更改默认模式，请设置 [`search_mode`](../configuration/config.md#search_mode)。

## 上下文切换

当你使用除 *global* 以外的过滤模式时，Atuin 会使用当前上下文（主机、会话、目录）来筛选历史记录。

你可以按 **ctrl-a** 再按 **c**，切换到当前选中命令所对应的上下文。

这会将过滤模式设置为 *session* 并清空搜索查询，从而显示同一 shell 会话中执行过的全部命令。

再次按下该组合键会返回初始上下文。你可以通过为 `switch-context` 和 `clear-context` 命令设置[自定义键位绑定](../configuration/advanced-key-binding.md)来自定义这一行为。`switch-context` 可以多次调用，以便在多个命令上下文之间导航，而 `clear-context` 则总是会返回初始上下文。
