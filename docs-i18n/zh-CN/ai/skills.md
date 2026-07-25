# Skills（技能）

Skills 是可复用的指令集——面向 Atuin AI 的行动手册、约定、工作流，或任何你希望 LLM 在执行特定任务时遵循的结构化指导。

## Skills 的工作原理

Skills 采用惰性加载：Atuin 仅将技能名称和描述发送给服务器，由 LLM 判断哪些技能相关，并按需加载其完整内容。你也可以在 TUI 中使用 `/skill-name` 直接调用技能。

## 创建技能

技能是一个目录，内含一个 `SKILL.md` 文件，该文件可带有可选的 YAML 前置数据（frontmatter）：

```
.atuin/skills/code-review/SKILL.md
```

```markdown
---
name: code-review
description: Conducts a structured code review. Use when the user asks to review code, a PR, or a diff.
---

When reviewing code:

1. **Correctness** — Does the code do what it claims?
2. **Edge cases** — What inputs could break it?
3. **Style** — Does it match the project's conventions?

Current branch: !`git branch --show-current`
```

## 技能位置

| 范围 | 路径 |
| ------- | ---------------------------------------- |
| 项目 | `.atuin/skills/<name>/SKILL.md`          |
| 全局 | `~/.config/atuin/skills/<name>/SKILL.md` |

如遇名称冲突，项目技能优先于全局技能。技能目录也支持嵌套，便于组织管理（例如 `.atuin/skills/ops/deploy/SKILL.md`）。

## 前置数据（Frontmatter）

所有前置数据字段均为可选。YAML 前置数据位于 `SKILL.md` 顶部的 `---` 标记之间。

| 字段 | 默认值 | 说明 |
| -------------------------- | ----------------------- | -------------------------------------------------------------------------------------------- |
| `name`                     | 目录名称 | 显示名称。仅可使用小写字母、数字和连字符。                                           |
| `description`              | 正文的第一段 | 该技能的用途说明。会发送给服务器，供 LLM 判断何时加载。                    |
| `disable-model-invocation` | `false`                 | 若为 `true`，LLM 将无法发现或加载该技能，只能通过 TUI 中的 `/name` 访问。   |

多行描述可以使用 YAML 的 `>`（折叠）或 `|`（字面量）语法编写。

## 调用技能

### 从 TUI 中

输入 `/skill-name` 即可直接调用某个技能，并支持 Tab 补全。也可以传入参数：

```
/deploy patch
```

LLM 会看到带有 `[Loaded skill: deploy]` 和 `[Arguments: patch]` 标头的技能内容。

### 由 LLM 调用

当 LLM 判断某个技能与你的请求相关时，它会自动调用 `load_skill` 来获取完整内容。Atuin 会将标记为 `disable-model-invocation: true` 的技能排除在外——LLM 不会看到它们。

## 动态内容

Skills 支持与[用户上下文文件](user-context.md)相同的 shell 替换方式：

- **内联（Inline）：** `!`command`` —— 替换为命令的 `stdout`
- **代码块（Block）：** ` ```! ` 代码块 —— 整个代码块替换为脚本的 `stdout`

命令会在技能加载时（即被调用时）运行，而非在发现时运行。

## 参数

当调用技能并附带参数时（例如 `/deploy patch`），技能正文中的 `$ARGUMENTS` 占位符会在 shell 替换运行之前被替换为参数字符串：

```yaml
---
name: deploy
description: Deploy the application
disable-model-invocation: true
---

Deploy $ARGUMENTS to production.
Current status: !`kubectl get deployment $ARGUMENTS`
```

如果正文中不包含 `$ARGUMENTS`，但你提供了参数，Atuin 会以 `ARGUMENTS: <value>` 的形式将参数追加到末尾。

## 描述预算

Atuin 会在总字符预算内向服务器发送技能描述：它会将每条描述截断至 1024 字节，跳过任何会导致超出预算的条目，并继续打包其余部分。如果因此遗漏了某些技能，会告知服务器具体是哪些。
