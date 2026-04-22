# Sending Additional Context in Atuin AI

Atuin AI allows you to send additional context to the LLM beyond just your prompt, similar to `AGENTS.md`.

## Additional Context Search Paths

Atuin AI looks for additional context in `.atuin/ai-context.md` files in the current directory and its parent directories. It also checks `ai-context.md` in your Atuin config directory (`~/.config/atuin/ai-context.md` by default). If it finds any of these files, it sends their contents as additional context to the LLM.

## Dynamic Content

You can send dynamic content by using shell substitution in your `ai-context.md` file:

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
