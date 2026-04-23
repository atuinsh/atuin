# Skills

Skills are reusable instruction sets for Atuin AI: playbooks, conventions, workflows, or any structured guidance you want the LLM to follow for specific tasks.

## How Skills Work

Skills are lazily loaded: Atuin sends only skill names and descriptions to the server. The LLM decides which skills are relevant and loads their full content on demand. You can also invoke skills directly with `/skill-name` in the TUI.

## Creating a Skill

A skill is a directory containing a `SKILL.md` file with optional YAML frontmatter:

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

## Skill Locations

| Scope   | Path                                     |
| ------- | ---------------------------------------- |
| Project | `.atuin/skills/<name>/SKILL.md`          |
| Global  | `~/.config/atuin/skills/<name>/SKILL.md` |

Project skills override global skills when names collide. Nested directories are supported for organization (e.g. `.atuin/skills/ops/deploy/SKILL.md`).

## Frontmatter

All frontmatter fields are optional. YAML frontmatter goes between `---` markers at the top of `SKILL.md`.

| Field                      | Default                 | Description                                                                                  |
| -------------------------- | ----------------------- | -------------------------------------------------------------------------------------------- |
| `name`                     | directory name          | Display name. Lowercase letters, numbers, hyphens.                                           |
| `description`              | first paragraph of body | What the skill does. Sent to the server so the LLM knows when to load it.                    |
| `disable-model-invocation` | `false`                 | If `true`, the LLM cannot discover or load the skill. Only reachable via `/name` in the TUI. |

Multiline descriptions using YAML's `>` (folded) or `|` (literal) syntax are supported.

## Invoking Skills

### From the TUI

Type `/skill-name` to invoke a skill directly. Tab-completion is available. Arguments are supported:

```
/deploy patch
```

The LLM will see the skill content with `[Loaded skill: deploy]` and `[Arguments: patch]` headers.

### By the LLM

When the LLM determines a skill is relevant to your request, it calls `load_skill` automatically to fetch the full content. Skills with `disable-model-invocation: true` are excluded from this — the LLM won't see them.

## Dynamic Content

Skills support the same shell substitution as [user context files](user-context.md):

- **Inline:** `!`command`` — replaced with command stdout
- **Block:** ` ```! ` code block — entire block replaced with script stdout

Commands run at skill load time (when invoked), not at discovery time.

## Arguments

When invoking a skill with arguments (e.g. `/deploy patch`), the `$ARGUMENTS` placeholder in the skill body is replaced with the argument string before shell substitution runs:

```yaml
---
name: deploy
description: Deploy the application
disable-model-invocation: true
---

Deploy $ARGUMENTS to production.
Current status: !`kubectl get deployment $ARGUMENTS`
```

If the body doesn't contain `$ARGUMENTS` and arguments were provided, they're appended as `ARGUMENTS: <value>`.

## Description Budget

Skill descriptions are packed into the request to the server under a total character budget. Each description is truncated at 512 characters, then skills are included until the budget is exhausted. If skills are omitted, the server is told which ones were left out.
