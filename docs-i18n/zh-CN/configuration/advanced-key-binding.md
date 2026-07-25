# 高级 Atuin UI 键位绑定

Atuin 提供一套功能强大的键位绑定系统，可用来完全自定义 TUI 的键盘快捷键。许多配置项，例如 `enter_accept`、`exit_past_line_start` 和 `accept_past_line_end`，都可以通过这套新配置显式表达出来。

配置文件中的 `[keymap]` 部分取代了旧的 `[keys]` 部分：只要存在任何 `[keymap]` 设置，`[keys]` 部分就会被完全忽略。

!!! warning
    修饰键、F1-F24 键以及一些特殊字符，只有在实现了 kitty 键盘协议的终端中才能达到最佳效果——某些情况下*只能*在这类终端中使用。需要特别说明的是，默认的 macOS Terminal 应用*并不*支持该协议。更多信息以及已知支持该协议的终端列表，请参阅 [https://sw.kovidgoyal.net/kitty/keyboard-protocol/](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)。

## 键位映射（Keymaps）

Atuin TUI 有多种模式，每种模式各自拥有独立的键位映射，你需要分别在对应的 TOML 表中配置：

| 配置部分 | 何时生效 |
|----------------------|-------------------|
| `[keymap.emacs]`     | 搜索标签页，`keymap_mode = "emacs"` |
| `[keymap.vim-normal]`| 搜索标签页，vim 普通模式（参见 [`keymap_mode`](config.md#keymap_mode)） |
| `[keymap.vim-insert]`| 搜索标签页，vim 插入模式（参见 [`keymap_mode`](config.md#keymap_mode)） |
| `[keymap.inspector]` | 检查器标签页（通过 `ctrl-o` 打开） |
| `[keymap.prefix]`    | 按下前缀键（默认是 `ctrl-a`）之后 |

vim 插入模式默认继承 Emacs 的全部键位绑定，再重写 `esc` 和 `ctrl-[`，使其进入普通模式而不是退出。

你只需指定想要修改的按键，未提及的按键会保留默认绑定。

!!! warning
    如果你在键位映射中指定了某个原本会被某项设置修改的按键——比如通过 `enter_accept` 设置修改 `enter` 键——该设置将不再生效。这类设置原本是根据自身取值去调整默认键位映射的，但一旦你在键位映射中覆盖了对应按键，就需要自行确保行为正确。

## 按键格式

按键以人类可读的形式指定为 TOML 字符串键。

### 基本按键

小写字母、数字与命名按键：

```
"a", "z", "1", "9"
"enter", "esc", "tab", "space", "backspace", "delete"
"up", "down", "left", "right"
"home", "end", "pageup", "pagedown"
"f1", "f2", ... "f12", ... "f24"
```

`return` 是 `enter` 的别名，`escape` 是 `esc` 的别名，`del` 是 `delete` 的别名。

!!! warning "macOS 的 delete 键"
    Mac 键盘上标有 "delete" 字样的按键，实际发送的是 `backspace`（删除的是光标*之前*的字符）。Atuin 中的 `delete` 键指的是向前删除，在 Mac 键盘上对应 `fn+delete`。

### 修饰键

修饰键以短横线分隔符作为前缀，可以组合多个修饰键：

```
"ctrl-c", "alt-f", "ctrl-alt-x"
```

可用的修饰键有：`ctrl`、`alt`、`shift`、`super`（也可写作 `cmd` 或 `win`）。

!!! warning
    `super` 修饰键（macOS 上的 `Cmd`，Windows 上的 Win）**依赖**kitty 键盘协议：只有实现该协议的终端才会向应用程序报告 Super 修饰键。即便在支持的终端中，某些 Super 组合键也可能被终端或操作系统拦截（例如 Cmd+C 用于复制、Cmd+V 用于粘贴，或 Cmd+T 用于打开新标签页）。

### 大写字母

大写字母代表其自身，无需附加 `shift` 修饰键。例如，`"G"` 匹配 `shift+g` 按键操作。

### 特殊字符

一些特殊字符可以直接写出：

```
"?", "/", "[", "]", "$"
```

### 带 Shift 的标点按键

当你按下类似 `Shift+1` 的按键时，终端发送的是结果字符（`!`），而不是 `shift-1`。要绑定带 Shift 的标点按键，请直接使用该字符：

```toml
[keymap.emacs]
"!" = "some-action"    # 绑定到 Shift+1
"@" = "some-action"    # 绑定到 Shift+2
"#" = "some-action"    # 绑定到 Shift+3
"$" = "cursor-end"     # 绑定到 Shift+4（vim 的 $ 动作）
```

任何单个字符都可以用作键位绑定。

!!! note
    对于非字符按键，例如 `"shift-tab"` 或 `"shift-up"`，`shift` 修饰键仍然有效。

### 媒体键

媒体键在实现了 kitty 键盘协议并启用了 `DISAMBIGUATE_ESCAPE_CODES` 的终端中受支持：

```
"play", "pause", "playpause", "stop"
"fastforward", "rewind", "tracknext", "trackprevious"
"record", "lowervolume", "raisevolume", "mutevolume", "mute"
```

### 多键序列

用空格分隔按键以定义一个序列。第一个按键会被缓冲，直至第二个按键到达：

```
"g g"
```

如果第二个按键无法构成已知序列，两个按键将分别单独处理。

## 键位映射格式

键位映射部分中的每一项都将一个按键映射到一个直接动作或一个条件规则列表。

### 直接绑定

将一个按键直接映射到单个动作，没有任何条件：

```toml
[keymap.emacs]
"ctrl-c" = "return-original"
"enter" = "accept"
```

### 条件绑定

将一个按键映射到一个有序的规则列表。每条规则都有一个 `action` 和一个可选的 `when` 条件。规则按从上到下的顺序进行评估，第一条条件匹配（或没有条件）的规则获胜。

```toml
[keymap.emacs]
"left" = [
  { when = "cursor-at-start", action = "exit" },
  { action = "cursor-left" },
]
```

在此示例中，当光标位于位置 0 时按下 left 键会退出 TUI。否则，它会将光标向左移动。

没有 `when` 字段的规则是无条件的，始终匹配。它通常放在最后作为兜底方案。

!!! warning "覆盖语义"
    当你在 `[keymap]` 中指定一个按键时，它会**替换**该按键**整个**默认绑定。你未提及的其他按键会保留其默认设置。

## 动作（Actions）

动作以 kebab-case 字符串形式指定。

### 光标移动

| 动作 | 描述 |
|--------|-------------|
| `cursor-left` | 将光标向左移动一个字符 |
| `cursor-right` | 将光标向右移动一个字符 |
| `cursor-word-left` | 将光标向左移动一个单词 |
| `cursor-word-right` | 将光标向右移动一个单词 |
| `cursor-word-end` | 将光标移动到当前/下一个单词的末尾（vim 的 `e` 动作） |
| `cursor-start` | 将光标移动到行首 |
| `cursor-end` | 将光标移动到行尾 |

### 编辑

| 动作 | 描述 |
|--------|-------------|
| `delete-char-before` | 删除光标之前的字符（退格） |
| `delete-char-after` | 删除光标之后的字符（删除） |
| `delete-word-before` | 删除光标之前的单词 |
| `delete-word-after` | 删除光标之后的单词 |
| `delete-to-word-boundary` | 删除到下一个单词边界（类似 `ctrl-w`） |
| `clear-line` | 清空整行输入 |
| `clear-to-start` | 清空输入行开头部分 |
| `clear-to-end` | 清空输入行结尾部分 |

### 列表导航

| 动作 | 描述 |
|--------|-------------|
| `select-next` | 将选择移动到结果列表中的下一项 |
| `select-previous` | 将选择移动到结果列表中的上一项 |
| `scroll-half-page-up` | 向上滚动半页 |
| `scroll-half-page-down` | 向下滚动半页 |
| `scroll-page-up` | 向上滚动一整页 |
| `scroll-page-down` | 向下滚动一整页 |
| `scroll-to-top` | 跳转到列表顶部 |
| `scroll-to-bottom` | 跳转到列表底部 |
| `scroll-to-screen-top` | 跳转到可见屏幕的顶部 |
| `scroll-to-screen-middle` | 跳转到可见屏幕的中间 |
| `scroll-to-screen-bottom` | 跳转到可见屏幕的底部 |

注意：`select-next` 和 `select-previous` 遵循 `invert` 设置。当 `invert` 为 true 时，视觉方向会被反转。

### 命令

| 动作 | 描述 |
|--------|-------------|
| `accept` | 接受所选条目并**立即执行** |
| `accept-N` | 接受所选项下方第 N 个条目并执行（例如 `accept-1` 到 `accept-9`） |
| `return-selection` | 将所选条目返回到命令行，**不执行** |
| `return-selection-N` | 返回所选项下方第 N 个条目而不执行（例如 `return-selection-1` 到 `return-selection-9`） |
| `return-original` | 关闭 TUI 并返回原始命令行文本 |
| `return-query` | 关闭 TUI 并返回当前搜索查询 |
| `copy` | 将所选条目复制到剪贴板 |
| `delete` | 从历史记录中删除所选条目 |
| `delete-all` | 删除与所选命令文本匹配的**所有**历史记录条目 |
| `exit` | 退出 TUI（行为取决于 `exit_mode` 设置） |
| `redraw` | 重新绘制屏幕 |
| `cycle-filter-mode` | 循环切换已启用的[过滤模式](config.md#filter_mode) |
| `cycle-search-mode` | 循环切换[搜索模式](config.md#search_mode)（fuzzy、prefix、fulltext、skim） |
| `toggle-tab` | 在搜索标签页和检查器标签页之间切换 |
| `switch-context` | 切换到当前所选命令的[上下文](../guide/advanced-usage.md#context-switch) |
| `clear-context` | 返回到初始[上下文](../guide/advanced-usage.md#context-switch) |

`accept` 与 `return-selection` 的区别在于：`accept` 会在 TUI 关闭时立即运行命令，而 `return-selection` 会将其放到命令行上，供你在按下回车之前进一步编辑。`enter_accept` 设置决定默认 `enter` 键使用这两者中的哪一个。

### 模式切换

| 动作 | 描述 |
|--------|-------------|
| `vim-enter-normal` | 切换到 vim 普通模式 |
| `vim-enter-insert` | 切换到 vim 插入模式（光标保持不动） |
| `vim-enter-insert-after` | 切换到 vim 插入模式（光标右移，类似 vim 的 `a`） |
| `vim-enter-insert-at-start` | 移动到行首并进入 vim 插入模式（类似 vim 的 `I`） |
| `vim-enter-insert-at-end` | 移动到行尾并进入 vim 插入模式（类似 vim 的 `A`） |
| `vim-search-insert` | 清空搜索输入并进入 vim 插入模式（类似 vim 的 `?` 或 `/`） |
| `vim-change-to-end` | 删除到行尾并进入 vim 插入模式（类似 vim 的 `C`） |
| `enter-prefix-mode` | 进入前缀模式（等待再输入一个按键，例如用 `d` 表示删除） |

### 检查器

| 动作 | 描述 |
|--------|-------------|
| `inspect-previous` | 检查上一个条目（在检查器标签页中） |
| `inspect-next` | 检查下一个条目（在检查器标签页中） |

### 特殊

| 动作 | 描述 |
|--------|-------------|
| `noop` | 不执行任何操作（适用于禁用默认绑定） |

## 条件（Conditions）

条件让同一个按键可以根据当前状态执行不同的操作。它们在规则的 `when` 字段中以字符串形式指定。

### 条件原子

| 条件 | 何时为真 |
|-----------|-----------|
| `cursor-at-start` | 光标位于位置 0 |
| `cursor-at-end` | 光标位于输入内容的末尾 |
| `input-empty` | 输入行为空（未输入任何文本） |
| `original-input-empty` | 传递给 TUI 的原始查询为空 |
| `list-at-start` | 选择位于第一个条目（索引 0） |
| `list-at-end` | 选择位于最后一个条目 |
| `no-results` | 搜索未返回任何结果 |
| `has-results` | 搜索至少返回一个结果 |
| `has-context` | 上下文来自之前所选的命令（`switch-context`） |

### 布尔表达式

条件支持具有标准优先级的布尔运算符（`!` 优先级最高，然后是 `&&`，最后是 `||`）。可以使用括号覆盖优先级。

```toml
# 取反
{ when = "!no-results", action = "select-next" }

# 与（AND）
{ when = "cursor-at-start && input-empty", action = "exit" }

# 或（OR）
{ when = "list-at-start || no-results", action = "exit" }

# 使用括号分组
{ when = "(cursor-at-start && !input-empty) || no-results", action = "return-original" }
```

## 示例

### 重现默认 `[keys]` 行为

默认键位映射本身就编码了标准的 `[keys]` 行为，以下展示的是它们对应的显式 `[keymap]` 写法，仅供参考。

**`scroll_exits = true`**（默认）—— 滚动越过第一个条目时退出：

```toml
[keymap.emacs]
"down" = [
  { when = "list-at-start", action = "exit" },
  { action = "select-next" },
]
```

**`exit_past_line_start = true`**（默认）—— 在位置 0 按下 left 时退出：

```toml
[keymap.emacs]
"left" = [
  { when = "cursor-at-start", action = "exit" },
  { action = "cursor-left" },
]
```

**`accept_past_line_end = true`**（默认）—— 在行尾按下 right 时接受：

```toml
[keymap.emacs]
"right" = [
  { when = "cursor-at-end", action = "accept" },
  { action = "cursor-right" },
]
```

**`accept_past_line_start = true`** —— 在位置 0 按下 left 时接受（默认关闭）：

```toml
[keymap.emacs]
"left" = [
  { when = "cursor-at-start", action = "accept" },
  { action = "cursor-left" },
]
```

**`accept_with_backspace = true`** —— 在输入为空时按下 backspace 时接受（默认关闭）：

```toml
[keymap.emacs]
"backspace" = [
  { when = "cursor-at-start", action = "accept" },
  { action = "delete-char-before" },
]
```

### 禁用滚动退出

要让 `down` 始终滚动而不会退出：

```toml
[keymap.emacs]
"down" = "select-next"
```

### 完全禁用某个按键

使用 `noop` 让某个按键不执行任何操作：

```toml
[keymap.emacs]
"ctrl-d" = "noop"
```

### ctrl-d 仅在输入为空时退出

```toml
[keymap.emacs]
"ctrl-d" = [
  { when = "input-empty", action = "exit" },
  { action = "delete-char-after" },
]
```

### 让 enter 返回所选内容而不执行

```toml
[keymap.emacs]
"enter" = "return-selection"
```

这等同于将 `enter_accept` 设置为 `false`，但直接以键位绑定的形式表达。

### 自定义 vim-normal 绑定

```toml
[keymap.vim-normal]
# 使用 'q' 退出
"q" = "exit"

# 使用 'x' 删除所选条目
"x" = "delete"

# 使用 'y' 复制
"y" = "copy"
```

### 自定义检查器绑定

```toml
[keymap.inspector]
# 在检查器中使用 'delete' 键移除条目
"delete" = "delete"
```

### 自定义前缀绑定

前缀模式是一种两步快捷方式：先按下前缀键（默认是 ++ctrl+a++），再按下第二个按键。这种方式很适合那些不必占用单个按键的动作。默认的前缀绑定如下：

| 按键 | 动作 |
|-----|--------|
| `d` | 删除所选条目 |
| `D` | 删除所有与所选命令匹配的条目 |
| `a` | 将光标移动到行首 |
| `c` | 清除上下文（如果处于已切换的上下文中），否则切换上下文 |

你可以通过 `[keymap.prefix]` 自定义这些绑定：

```toml
[keymap.prefix]
# 添加一个用于复制所选条目的绑定
"y" = "copy"

# 让 'x' 执行删除操作，而不是 'd'
"x" = "delete"
"d" = "noop"
```

要更改进入前缀模式的按键，请在 `[keys]` 下设置 `prefix`：

```toml
[keys]
prefix = "x"  # 使用 ctrl-x 代替 ctrl-a
```

或者直接在键位映射中绑定 `enter-prefix-mode`：

```toml
[keymap.emacs]
"ctrl-x" = "enter-prefix-mode"
```

## 与 `[keys]` 的关系

`[keymap]` 部分是 `[keys]` 部分功能更强大的替代品。两者是**互斥**的：

- 如果你有任何 `[keymap]` 设置，整个 `[keys]` 部分都会被忽略。默认设置会先根据标准的 `[keys]` 值构建，再应用你的 `[keymap]` 覆盖项。
- 如果你没有任何 `[keymap]` 设置，`[keys]` 部分将像以前一样工作，以保持向后兼容。

如果你正在从 `[keys]` 迁移到 `[keymap]`，以下是旧标志的对应关系：

| `[keys]` 设置 | 等效的 `[keymap]` |
|------------------|-----------------------|
| `scroll_exits = false` | 在相应的键位映射中设置 `"down" = "select-next"` 和 `"up" = "select-previous"` |
| `exit_past_line_start = false` | `"left" = "cursor-left"` |
| `accept_past_line_end = false` | `"right" = "cursor-right"` |
| `accept_past_line_start = true` | `"left" = [{ when = "cursor-at-start", action = "accept" }, { action = "cursor-left" }]` |
| `accept_with_backspace = true` | `"backspace" = [{ when = "cursor-at-start", action = "accept" }, { action = "delete-char-before" }]` |
| `prefix = "x"` | 前缀键变为 `ctrl-x`（在 emacs/vim 的键位映射中设置） |
