# 在 Atuin AI 中发送额外上下文

借助 Atuin AI，你可以在提示词之外向 LLM 发送额外的上下文，作用类似于 `CLAUDE.md` 或 `AGENTS.md`。

## 额外上下文的搜索路径

Atuin AI 会在当前目录及其所有父目录中查找额外上下文，在每个目录下都会检查两个位置：

- `.atuin/TERMINAL.md` —— `.atuin` dotdir 内的专属文件
- `TERMINAL.md` —— 直接位于该目录下（例如项目根目录）

它还会检查你的 Atuin 配置目录下的 `TERMINAL.md`（默认路径为 `~/.config/atuin/TERMINAL.md`）。

只要找到了其中任意文件，它就会将其内容作为额外上下文发送给 LLM。Atuin AI 最多会发送 10 个额外上下文文件：全局范围内找到的文件优先发送，其余文件按文件系统深度由浅到深依次发送，且每个文件最多发送 10,000 个字符。

## 动态内容

你可以在 `TERMINAL.md` 文件中使用 shell 替换来发送动态内容：

```markdown
My username: !`whoami`
```

当 Atuin AI 读取此文件时，它会执行 `whoami` 命令，并将其输出包含在发送给 LLM 的上下文中。如果你的用户名是 `binarymuse`，发送给 LLM 的上下文将包含：

```markdown
My username: binarymuse
```

Atuin AI 也可以对代码块执行替换，以运行多行命令。例如：

````markdown
```!
node --version
npm --version
git status --short
```
````

## 缓存

Atuin AI 首次加载 `TERMINAL.md` 文件后会对其进行缓存，因此如果你在会话中途修改了这些文件，请使用 `/reload` 斜杠命令来刷新数据。这会使下一次请求时的服务器缓存失效，从而增加该次请求的延迟和 token 用量。

## 为什么不使用 `AGENTS.md`？

大多数代理文件都是针对 _编码_ 代理优化的：模式、工具、编码风格等等。这对编码代理来说非常合适，但对通用型代理来说用处就没那么大了。通过改用 `TERMINAL.md`，Atuin AI 提供了一种更灵活的方式来发送额外上下文，不必受限于编码相关的特定模式，用户可以按自己的需要提供任何上下文，而不受代理文件结构的约束。

如果你的代理文件中包含相关信息，你可以在 `TERMINAL.md` 中指示 LLM 从中读取。
