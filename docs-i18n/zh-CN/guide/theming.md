# 主题

在终端界面自定义方面，Atuin 同时支持用户主题和内置颜色主题。

Atuin 内置的主题不多，但可以通过 TOML 文件添加更多主题。

## 必需配置

以下内容是配置文件（`~/.config/atuin/config.toml`）中的必需配置：

```toml
[theme]
name = "THEMENAME"
```

其中 `THEMENAME` 为已知主题名称。以下主题开箱即用：

* `default` 主题
* `autumn` 主题
* `marine` 主题
* `(none)` 主题（移除所有样式）

这些主题的存在是为了让用户和开发者能够试用主题功能，但在实际使用中，你通常需要下载主题或自行制作主题。

如果你正在编写自己的主题，可以在同一配置块中添加下面这一行以获取额外的输出：

```toml
debug = true
```

这会打印出所有无法从所请求主题解析出的颜色名称。

此外还有一个可选设置：

```toml
max_depth = 10
```

它用于设置遍历主题父级链的最大层数，正常使用时通常无需显式添加此设置。

## 用法

### 主题结构

主题是从 *Meaning*（含义）到颜色的映射（目前仅限于颜色），*Meaning* 用于描述开发者的意图。未来这一机制可能会扩展，以支持更丰富的样式。

*Meaning* 取自一个枚举类型，包含以下取值：

* `AlertInfo`：以 INFO 级别提醒用户
* `AlertWarn`：以 WARN 级别提醒用户
* `AlertError`：以 ERROR 级别提醒用户
* `Annotation`：重要性较低的辅助性文本
* `Base`：默认前景色
* `Guidance`：以帮助或上下文形式指导用户
* `Important`：提醒用户注意某条信息
* `Title`：为某个区域或视图添加标题
* `Muted`：一种低调、通常为灰色的前景色，用于与其他颜色形成对比。该值通常与 Base 颜色相同，但主题可以单独更改 Base 颜色，从而降低破坏预期颜色对比度（例如堆叠柱状图）的风险
* `SyntaxCommand`：对 shell 命令进行语法高亮时的命令词（`git status` 中的 `git`）
* `SyntaxFlag`：`-f`/`--flag` 这类参数
* `SyntaxString`：带引号的字符串
* `SyntaxVariable`：`$VAR`、`${VAR}` 或 `FOO=bar` 赋值
* `SyntaxOperator`：诸如 `|`、`&&`、`;`、`>` 等运算符
* `SyntaxComment`：`# comment` 注释

随着 Atuin 代码库的演进，这些取值可能会不断增加。对于任何新增的 *Meaning*，Atuin 都应当提供回退方案，这样无论主题只涵盖当前列表，还是后续用到了新的 *Meaning*，都能正常工作。

**致 Atuin 贡献者**：如有需要，请在自己的 PR 中识别并酌情扩展 Meaning 枚举（别忘了同时提供一个回退 Meaning！）。

### 创建主题

当遇到尚未加载的主题名称时，Atuin 会在 `~/.config/atuin/themes/` 文件夹中查找该主题，除非这个文件夹已被 `ATUIN_THEME_DIR` 环境变量覆盖。它会尝试打开名为 `THEMENAME.toml` 的文件，并将其解析为一个从 *Meaning* 到前景色的映射。

请注意，目前还无法在主题文件中显式指定默认终端颜色。不过，默认主题的 Base 颜色始终不会被设置，因此会呈现为用户的默认终端颜色。因此，只有当你的主题在其他情况下没有意义时，才应该在自己的主题中覆盖 Base 颜色，或者从一个已经覆盖了 Base 颜色的主题派生。比如，`marine` 主题旨在让所有内容都呈现绿色或蓝色调，因此覆盖了 Base 颜色；而 `autumn` 主题只是想让自定义颜色更显暖调，因此并未覆盖 Base 颜色。

颜色既可以使用 [palette](https://ogeon.github.io/docs/palette/master/palette/named/index.html) crate 中的小写名称指定，也可以指定为以 `#` 为前缀的六位十六进制代码。如果想通过整数显式选择 ANSI 颜色，或者需要更高的灵活性，可以为颜色加上 `@` 前缀，字符串的其余部分会交给 Crossterm 用它自身的颜色解析逻辑处理。相关示例请参见 [crossterm 的颜色反序列化测试](https://github.com/crossterm-rs/crossterm/blob/5d50d8da62c5e034ef8b2787a771a2c0f9b3b2f9/src/style/types/color.rs#L389)，注意在 Atuin 中仍需加上 `@` 前缀。

例如，以下都是合法的颜色名称：

* `#ff0088`
* `teal`
* `powderblue`
* `@ansi_(255)`
* `@rgb_(255, 128, 0)`

你也可以使用 Crossterm 支持的字符串来表示颜色，同样需要加上 `@` 前缀。例如，

* `@ansi_(123)`
* `@dark_yellow`

目前虽然还没有官方参考文档，但你可以参考 [crossterm 测试](https://docs.rs/crossterm/latest/src/crossterm/style/types/color.rs.html#376) 中的示例。由于这些值会被原样传递给 Crossterm，使用 [ANSI 代码](https://www.ditig.com/256-colors-cheat-sheet) 有助于确保你的主题兼容 256 色终端。

接下来就可以构建一个主题文件了，例如 `my-theme.toml`：

```toml
[theme]
name = "my-theme"
parent = "autumn"

[colors]
AlertInfo = "green"
Guidance = "#888844"

```

这里不需要显式定义所有的 *Meaning*。如果某个 *Meaning* 缺失，其颜色会从父主题中选取（前提是定义了父主题）；如果 `theme` 块中没有这个键，则会从 `default` 主题中选取。

如果指定的主题名称根本不存在，则会报错。此时主题会回退到 `(none)`，让 Atuin 保持无样式状态，而不是回退到 default 主题或其他任何主题。

应将该主题文件移动到 `~/.config/atuin/themes/my-theme.toml`，并在 `~/.config/atuin/config.toml` 中添加以下内容：

```toml
[theme]
name = "my-theme"
```

下次运行 Atuin 时，你的主题就会生效。
