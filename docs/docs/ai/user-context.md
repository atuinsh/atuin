# Sending Additional Context in Atuin AI

Atuin AI allows you to send additional context to the LLM beyond just your prompt, similar to `CLAUDE.md` or `AGENTS.md`.

## Additional Context Search Paths

Atuin AI looks for additional context in `TERMINAL.md` and `.atuin/TERMINAL.md` files in the current directory and its parent directories. It also checks `TERMINAL.md` in your Atuin config directory (`~/.config/atuin/TERMINAL.md` by default). If it finds any of these files, it sends their contents as additional context to the LLM.

- `.atuin/TERMINAL.md` — scoped inside the `.atuin` dotdir
- `TERMINAL.md` — at the directory root (e.g. project root)

It also checks `TERMINAL.md` in your Atuin config directory (`~/.config/atuin/TERMINAL.md` by default).

If it finds any of these files, it sends their contents as additional context to the LLM. Atuin AI will send at maximum 10 additional context files, prioritizing files found globally first and then other files in order of filesystem depth, shallowest to deepest, and each file is limited to 10,000 characters.

## Dynamic Content

You can send dynamic content by using shell substitution in your `TERMINAL.md` file:

```markdown
My username: !`whoami`
```

When Atuin AI reads this file, it will execute the `whoami` command and include its output in the context sent to the LLM. So if your username is `binarymuse`, the context sent to the LLM would include:

```markdown
My username: binarymuse
```

Atuin AI can also run substitutions for code blocks, to run multi-line commands. For example:

````markdown
```!
node --version
npm --version
git status --short
```
````

## Why not `AGENTS.md`?

Most agent files are optimized for _coding_ agents: patterns, tools, coding style, and so on. This is great for coding agents, but not as useful for general-purpose agents. By using `TERMINAL.md` instead, we can provide a more flexible way to send additional context that is not tied to coding-specific patterns. This allows users to provide any kind of context they want, without being constrained by the structure of an agent file.

If your agent file has relevant information, you can instruct the LLM in `TERMINAL.md` to read from it.
