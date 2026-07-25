# Atuin zh-CN 术语表 (Glossary)

This is the **single source of truth** for how Atuin terminology is rendered in
Simplified Chinese. Every team member — translator, technical writer, and judge
— follows this table so that separate files (and separate runs) stay consistent.

It was seeded from the existing translations in `docs-i18n/zh-CN/`. When you
encounter a recurring term that isn't here, translate it sensibly, stay
consistent within the document, and note it so the table can grow.

## Rendering rules

- **Keep in English, do not translate:** `Atuin`, `shell`, `bash`, `zsh`,
  `fish`, `nu`, `SQLite`, `cwd`, `Discord`, product/brand names, CLI commands
  (`atuin sync`), flags (`--exit`), env vars (`ATUIN_CONFIG_DIR`), file paths,
  and anything inside code blocks or inline `code`.
- **Keyboard keys** stay as written (`++ctrl+r++`, `<kbd>Ctrl-r</kbd>`, `up`).
- A term like "shell 历史记录" mixes an English word and Chinese — that is the
  house style here, not a mistake. Don't "fix" `shell` into 外壳.

## Core terms

| English | zh-CN | Notes |
|---|---|---|
| shell history | shell 历史记录 | often shortened to 历史记录 in running text |
| history | 历史记录 | |
| sync | 同步 | verb and noun |
| sync server | 同步服务器 | |
| end-to-end encrypted | 端到端加密 | |
| encrypted / encryption | 加密 | |
| exit code | 退出代码 | |
| command duration | 命令持续时间 | |
| key binding | 键位绑定 | |
| configuration / config | 配置 | |
| config file | 配置文件 | |
| client | 客户端 | |
| server | 服务器 | |
| self-hosting | 自托管 | |
| import | 导入 | |
| session | 会话 | |
| directory | 目录 | |
| working directory (cwd) | 工作目录 | keep `cwd` when the source does |
| hostname | 主机名 | |
| shell plugin | shell 插件 | |
| shell integration | shell 集成 | |
| stats / statistics | 统计数据 | |
| register | 注册 | |
| login | 登录 | |
| workspace | 工作区 | |
| dotfiles | dotfiles | keep in English |
| daemon | 守护进程 | |
| search mode | 搜索模式 | |
| filter mode | 过滤模式 | |
| record | 记录 | |
| command | 命令 | |
| documentation / docs | 文档 | |
| installation script | 安装脚本 | |
| package manager | 包管理器 | |

## Punctuation & formatting

- Use full-width Chinese punctuation in prose: `，。：；？！（）「」`. Do **not**
  use full-width punctuation inside code, paths, commands, or English fragments.
- Keep one space between Chinese text and adjacent Latin words/numbers
  (e.g. `使用 SQLite 数据库`, `按 ++ctrl+r++ 搜索`). This matches the existing
  translations and improves readability.
- Preserve the source's Markdown exactly: heading levels, list markers, tables,
  links, image tags, admonition keywords (`!!! warning`), and blank lines.

## Anchor-bearing headings (important)

In-page links like `[register](#register)` resolve to an anchor that mkdocs
auto-generates from the heading text. If you translate `## Register` to
`## 注册`, the anchor becomes `#注册` and the `#register` link **breaks**.

Don't leave the heading in English to work around this — a Chinese doc with
stray English section titles reads badly. Instead, translate the heading **and**
pin the original anchor with `attr_list` syntax (enabled in `docs/mkdocs.yml`):

```
## 注册 {#register}
```

So the reader sees a Chinese heading and every `#register` / `#login` link still
lands. Scan the file for in-page links (`](#...)`) first, then add `{#anchor}` to
each heading they point at, using the anchor exactly as the link spells it.
