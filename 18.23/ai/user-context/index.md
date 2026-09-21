# Sending Additional Context in Atuin AI

Use Atuin AI to send additional context to the LLM beyond just your prompt, similar to `CLAUDE.md` or `AGENTS.md`.

## Additional Context Search Paths

Atuin AI looks for additional context in the current directory and its parent directories, in two locations per directory:

- `.atuin/TERMINAL.md` — scoped inside the `.atuin` dotdir
- `TERMINAL.md` — at the directory root (for example, project root)

It also checks `TERMINAL.md` in your Atuin config directory (`~/.config/atuin/TERMINAL.md` by default).

If it finds any of these files, it sends their contents as additional context to the LLM. Atuin AI will send at maximum 10 additional context files, prioritizing files found globally first and then other files in order of filesystem depth, shallowest to deepest, and each file is limited to 10,000 characters.

## Dynamic Content

You can send dynamic content by using shell substitution in your `TERMINAL.md` file:

```
My username: !`whoami`
```

When Atuin AI reads this file, it will execute the `whoami` command and include its output in the context sent to the LLM. If your username is `binarymuse`, the context sent to the LLM would include:

```
My username: binarymuse
```

Atuin AI can also run substitutions for code blocks, to run multi-line commands. For example:

````
```!
node --version
npm --version
git status --short
````

````

## Caching

Atuin AI caches `TERMINAL.md` files after it first loads them, so if you change them mid-session, use the `/reload` slash command to refresh the data. This will invalidate the server cache on the next request, increasing the latency and token usage for that request.

## Why not `AGENTS.md`?

Most agent files are optimized for *coding* agents: patterns, tools, coding style, and so on. This is great for coding agents, but not as useful for general-purpose agents. By using `TERMINAL.md` instead, Atuin AI provides a more flexible way to send additional context that's not tied to coding-specific patterns. Users can then provide any kind of context they want, without being constrained by the structure of an agent file.

If your agent file has relevant information, you can instruct the LLM in `TERMINAL.md` to read from it.```
````
