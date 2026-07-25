# 键位绑定

## 自定义上箭头过滤模式

为上箭头键设置不同的过滤或搜索模式，有时会很有用。例如，你可以用 ctrl-r 进行全局搜索，而用上箭头仅搜索当前目录的历史记录。

按如下方式配置：

```toml
filter_mode_shell_up_key_binding = "directory" # or global, host, directory, etc
```

## 禁用上箭头

我们默认的上箭头绑定颇具争议：有人喜欢它，有人讨厌它。许多起初觉得别扭的人后来反而爱上了它，所以不妨亲自试试看！

如果你为上箭头绑定不同的过滤模式，它会变得更加强大。例如，按下"上箭头"时，Atuin 可以默认仅搜索当前目录的全部历史记录，而 Ctrl-r 则进行全局历史记录搜索。更多内容请参阅[配置](config.md#filter_mode_shell_up_key_binding)。

如果你不喜欢它，也可以将其禁用。

你也可以单独禁用上箭头或 ++ctrl+r++ 绑定，方法是在 shell 配置文件中调用 `atuin init` 时传入
`--disable-up-arrow` 或 `--disable-ctrl-r`：

一个 zsh 的示例：
```shell
# 绑定 ctrl-r，但不绑定上箭头
eval "$(atuin init zsh --disable-up-arrow)"

# 绑定上箭头，但不绑定 ctrl-r
eval "$(atuin init zsh --disable-ctrl-r)"
```

如果这两个按键都不想绑定，可以同时传入两个 `--disable` 参数，或者在调用
`atuin init` 之前将环境变量 `ATUIN_NOBIND` 设置为任意值：

```shell
## 不绑定任何按键
# 方式一：
eval "$(atuin init zsh --disable-up-arrow --disable-ctrl-r)"

# 方式二：
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"
```

之后，你可以根据需要自行绑定 Atuin，只需在调用 init 之后进行即可。

## 回车键行为

默认情况下，`enter` 键会直接执行选中的命令，而不像 `tab` 键那样让你先编辑它。如果你想更改此行为，请在配置中设置 `enter_accept = false`。更多详情请参阅[`enter_accept`](config.md#enter_accept)。

## Ctrl-n 键快捷方式

macOS 没有 ++alt++ 键，不过终端模拟器通常可以配置为将 ++option++ 键映射为 ++alt++ 使用。*然而*，这样重新映射 ++option++ 可能会导致某些字符无法输入，例如在英式英语键盘布局下，++option+3++ 用于输入 `#`。针对这种情况，可以在配置文件中将 `ctrl_n_shortcuts` 选项设置为 `true`，从而将 ++alt+0++ 到 ++alt+9++ 的快捷方式替换为 ++ctrl+0++ 到 ++ctrl+9++：

```toml
# Use Ctrl-0 .. Ctrl-9 instead of Alt-0 .. Alt-9 UI shortcuts
ctrl_n_shortcuts = true
```

Linux 上的 Ghostty 将 ++alt+1++ .. ++alt+9++ 映射为按编号切换标签页。要禁用此行为，可以在 `~/.config/ghostty/config` 中添加以下内容：
```ini
keybind=alt+one=unbind
keybind=alt+two=unbind
keybind=alt+three=unbind
keybind=alt+four=unbind
keybind=alt+five=unbind
keybind=alt+six=unbind
keybind=alt+seven=unbind
keybind=alt+eight=unbind
keybind=alt+nine=unbind
```
（这将禁用通过 ++alt+n++ 切换标签页的功能）
或者按照上文所述使用 `ctrl_n_shortcuts`。

## zsh

如果你想进一步自定义按键绑定，可以通过自定义 shell 配置来实现：

Atuin 定义了 ZLE 组件 `atuin-search` 和 `atuin-up-search`。后者可用于绑定 ++up++ 键及类似按键。

注意：在 `atuin < 18.0` 中，请分别改用组件名称 `_atuin_search_widget` 和 `_atuin_up_search_widget`。

```shell
export ATUIN_NOBIND="true"
eval "$(atuin init zsh)"

bindkey '^r' atuin-search

# 绑定上箭头键，具体按键取决于终端模式
bindkey '^[[A' atuin-up-search
bindkey '^[OA' atuin-up-search
```

对于 vi 模式下的按键绑定，可以将 `atuin-search-viins`、`atuin-search-vicmd`、`atuin-up-search-viins` 和 `atuin-up-search-vicmd`（`atuin >= 18.0`）与配置项 [`keymap_mode`](config.md#keymap_mode)（`atuin >= 18.0`）结合使用，以便在相应的键盘映射模式下启动 Atuin 搜索。

## bash

Atuin（`>= 18.10.0`）提供了一个 shell 函数 `atuin-bind` 用于设置按键绑定：

```shell
atuin-bind [-m KEYMAP] KEYSEQ COMMAND
```

`KEYMAP` 为 `emacs`、`vi-insert`、`vi-command` 三者之一，用于指定该按键绑定所应用的键盘映射。`KEYSEQ` 按照 `bind '"KEYSEQ": ...'` 所用的格式指定按键序列，`COMMAND` 指定该按键绑定要运行的 shell 命令。除下表中的特殊命令外，你也可以使用任意 shell 命令：

| 命令                     | 说明                                                                                 |
| ------------------------ | ------------------------------------------------------------------------------------ |
| `atuin-search`          | 标准搜索                                                                             |
| `atuin-search-emacs`    | 使用 `emacs` 键盘映射模式的标准搜索                                                  |
| `atuin-search-viins`    | 使用 `vim-insert` 键盘映射模式的标准搜索                                             |
| `atuin-search-vicmd`    | 使用 `vim-normal` 键盘映射模式的标准搜索                                             |
| `atuin-up-search`       | 用于 <kbd>up</kbd> 或类似按键的搜索命令                                              |
| `atuin-up-search-emacs` | 用于 <kbd>up</kbd> 或类似按键的搜索命令，使用 `emacs` 键盘映射模式                   |
| `atuin-up-search-viins` | 用于 <kbd>up</kbd> 或类似按键的搜索命令，使用 `vim-insert` 键盘映射模式              |
| `atuin-up-search-vicmd` | 用于 <kbd>up</kbd> 或类似按键的搜索命令，使用 `vim-normal` 键盘映射模式              |

键盘映射模式用于控制 Atuin 搜索中的初始键盘映射，具体由该模式与配置项 [`keymap_mode`](config.md#keymap_mode)（`atuin >= 18.0`）共同决定。


```shell
export ATUIN_NOBIND="true"
eval "$(atuin init bash)"

# 绑定 ctrl-r，你也可以在这里添加其他绑定
atuin-bind '\C-r' atuin-search
# CTRL + 上箭头键的示例
# atuin-bind '\e[1;5A' atuin-search

# 绑定上箭头键，具体按键取决于终端模式
atuin-bind '\e[A' atuin-up-search
atuin-bind '\eOA' atuin-up-search
```

在较旧版本的 Atuin 中，用户需要使用 Bash 的 `bind` 直接绑定一个可绑定的 shell 函数 `__atuin_history`。对于 <kbd>up</kbd> 键或类似按键的绑定，可以在第一个参数中可选地指定标志 `--shell-up-key-binding`。对于 `vi` 编辑模式下的按键绑定，可以为 shell 函数 `__atuin_history` 额外指定选项 `--keymap-mode=vim-insert`，以及用于键盘映射模式的 `--keymap-mode=vim-normal`（`atuin >= 18.0`）。

## fish
在 ~/.config/fish/config.fish 中添加以下内容，即可编辑 fish shell 中的按键绑定

```shell
set -gx ATUIN_NOBIND "true"
atuin init fish | source

# 在普通模式和插入模式下绑定 ctrl-r，你也可以在这里添加其他绑定
bind \cr _atuin_search
bind -M insert \cr _atuin_search
```

对于 ++up++ 按键绑定，可以使用 `_atuin_bind_up` 代替 `_atuin_search`。

添加实用的备用按键绑定 ++ctrl+up++ 相对棘手，具体取决于终端对 terminfo(5) 的遵循程度。

方便的是，fish 提供了一个命令，可以捕获按键并告诉你针对当前终端应添加的确切命令：在终端中运行 `fish_key_reader`，然后按下你想要的按键。

例如，在 Gnome 终端中，++ctrl+up++ 对应的输出是 `bind \e\[1\;5A 'do something'`

将其添加到上面的示例中，`bind \e\[1\;5A _atuin_search` 将提供额外的搜索按键绑定。

## nu

```
$env.ATUIN_NOBIND = true
atuin init nu | save -f ~/.local/share/atuin/init.nu #请确保事先已使用 `mkdir ~/.local/share/atuin` 创建了该目录
source ~/.local/share/atuin/init.nu

#在 emacs、vi_normal 和 vi_insert 模式下绑定 ctrl-r，你也可以在这里添加其他绑定
$env.config = (
    $env.config | upsert keybindings (
        $env.config.keybindings
        | append {
            name: atuin
            modifier: control
            keycode: char_r
            mode: [emacs, vi_normal, vi_insert]
            event: { send: executehostcommand cmd: (_atuin_search_cmd) }
        }
    )
)
```


## Atuin 界面快捷键

| 快捷键                                     | 操作                                                                            |
|---------------------------------------------|---------------------------------------------------------------------------------|
| Enter                                       | 执行选中的项目                                                                   |
| Tab                                         | 选中项目并编辑                                                                   |
| Ctrl + r                                    | 循环切换过滤模式                                                                 |
| Ctrl + s                                    | 循环切换搜索模式                                                                 |
| Alt + 1 到 Alt + 9                          | 通过其旁边显示的数字选中项目                                                     |
| Ctrl + c / Ctrl + d / Ctrl + g / Esc        | 返回原始状态                                                                     |
| Ctrl + y                                    | 将选中的项目复制到剪贴板                                                         |
| Ctrl + ← / Alt + b                          | 将光标移动到上一个单词                                                           |
| Ctrl + → / Alt + f                          | 将光标移动到下一个单词                                                           |
| Ctrl + b / ←                                | 将光标向左移动                                                                   |
| Ctrl + f / →                                | 将光标向右移动                                                                   |
| Ctrl + a / Home                             | 将光标移动到行首                                                                 |
| Ctrl + e / End                              | 将光标移动到行尾                                                                 |
| Ctrl + Backspace / Ctrl + Alt + Backspace   | 删除上一个单词／删除光标之前的单词                                               |
| Ctrl + Delete / Ctrl + Alt + Delete         | 删除下一个单词或光标之后的单词                                                   |
| Ctrl + w                                    | 删除光标之前的单词，即使它跨越了单词边界                                         |
| Ctrl + u                                    | 清空当前行                                                                       |
| Ctrl + n / Ctrl + j / ↓                     | 选中列表中的下一个项目                                                           |
| Ctrl + p / Ctrl + k / ↑                     | 选中列表中的上一个项目                                                           |
| Ctrl + o                                    | 打开[检查器](#inspector)                                                        |
| Page Down                                   | 将搜索结果向下滚动一页                                                           |
| Page Up                                     | 将搜索结果向上滚动一页                                                           |
| ↓（位于第一条时）                           | 根据[设置](config.md#exit_mode)返回原始状态或返回查询内容                       |
| Ctrl + a, d                                 | 删除选中的历史记录条目                                                           |
| Ctrl + a, D                                 | 删除与选中命令匹配的**所有**历史记录条目                                         |
| Ctrl + a, a                                 | 将光标移动到行首                                                                 |
| Ctrl + a, c                                 | 切换到当前选中命令的上下文／返回默认状态                                         |

### 前缀模式

上面以 ++ctrl+a++ 开头的快捷键使用**前缀模式（prefix mode）**——一种两步式的按键组合。按下前缀键（默认是 ++ctrl+a++）会进入前缀模式，然后你按下的下一个键会触发相应操作。操作执行完毕后，前缀模式会自动退出。

这对于不需要专用快捷键的低频操作很有用。前缀键可以通过 [`prefix`](config.md#prefix) 设置进行更改，绑定本身也可以通过 [`[keymap.prefix]`](advanced-key-binding.md#custom-prefix-bindings) 进行自定义。

### Vim 模式
如果在配置中启用了 vim（参见 [`keymap_mode`](config.md#keymap_mode)），则会启用以下按键绑定：

| 快捷键   | 模式   | 操作                                        |
| -------- | ------ | ------------------------------------------- |
| k        | 普通模式 | 选中列表中的上一个项目                     |
| j        | 普通模式 | 选中列表中的下一个项目                     |
| h        | 普通模式 | 将光标向左移动                             |
| l        | 普通模式 | 将光标向右移动                             |
| 0        | 普通模式 | 将光标移动到行首                           |
| $        | 普通模式 | 将光标移动到行尾                           |
| w        | 普通模式 | 将光标移动到下一个单词                     |
| b        | 普通模式 | 将光标移动到上一个单词                     |
| e        | 普通模式 | 将光标移动到当前／下一个单词的末尾         |
| x        | 普通模式 | 删除光标处的字符                           |
| dd       | 普通模式 | 清空整行                                   |
| D        | 普通模式 | 删除到行尾                                 |
| C        | 普通模式 | 删除到行尾并进入插入模式                   |
| i        | 普通模式 | 进入插入模式                               |
| I        | 普通模式 | 移动到行首并进入插入模式                   |
| a        | 普通模式 | 向右移动并进入插入模式                     |
| A        | 普通模式 | 移动到行尾并进入插入模式                   |
| Ctrl+u   | 普通模式 | 向上翻半页（朝可视区域顶部）               |
| Ctrl+d   | 普通模式 | 向下翻半页（朝可视区域底部）               |
| Ctrl+b   | 普通模式 | 向上翻整页（朝可视区域顶部）               |
| Ctrl+f   | 普通模式 | 向下翻整页（朝可视区域底部）               |
| G        | 普通模式 | 跳转到历史记录的可视区域底部               |
| `gg`     | 普通模式 | 跳转到历史记录的可视区域顶部               |
| H        | 普通模式 | 跳转到可见屏幕的顶部                       |
| M        | 普通模式 | 跳转到可见屏幕的中间                       |
| L        | 普通模式 | 跳转到可见屏幕的底部                       |
| ? 或 /   | 普通模式 | 清空输入并进入插入模式                     |
| 1-9      | 普通模式 | 按编号选中项目                             |
| Enter    | 普通模式 | 执行选中的项目（遵循 `enter_accept`）      |
| Esc      | 插入模式 | 进入普通模式                               |


### 检查器 {#inspector}
通过 Ctrl + o 打开检查器

| 快捷键     | 操作                                          |
| ---------- | --------------------------------------------- |
| Esc        | 关闭检查器，返回 shell                        |
| Ctrl + o   | 关闭检查器，返回搜索视图                      |
| Ctrl + d   | 从历史记录中删除被检查的项目                  |
| ↑          | 检查历史记录中的上一个项目                    |
| ↓          | 检查历史记录中的下一个项目                    |
| Page Up    | 检查历史记录中的上一个项目                    |
| Page Down  | 检查历史记录中的下一个项目                    |
| j / k      | 在启用 vim 模式时浏览项目                     |
| Enter      | 执行选中的项目（遵循 `enter_accept`）         |
| Tab        | 选中当前项目并编辑                            |
