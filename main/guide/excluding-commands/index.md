# Keeping Secrets Out of Atuin

Atuin records your commands, and can also record [what they print](https://docs.atuin.sh/guide/output-capture/index.md) and your [AI agents' sessions](https://docs.atuin.sh/ai/sessions/index.md). This page covers how to keep secrets, and anything else you don't want stored, out of all three.

| You want to keep out                       | Use                                                    |
| ------------------------------------------ | ------------------------------------------------------ |
| One command, just this once                | [A leading space](#one-command-with-a-leading-space)   |
| Commands matching a pattern                | [`history_filter`](#commands-history_filter)           |
| Everything run in a directory              | [`cwd_filter`](#a-directory-cwd_filter)                |
| What a command prints, but not the command | [`command_filter`](#a-commands-output-command_filter)  |
| The contents of a file, read or written    | [`sensitive_files`](#a-files-contents-sensitive_files) |
| A secret of your own, wherever it appears  | [`redact_patterns`](#your-own-secrets-redact_patterns) |
| What your AI agents' tools read and run    | [`capture_tools`](#ai-agents-tool-calls-capture_tools) |
| Everything from a particular tool          | [Skip Atuin for it](#everything-from-a-tool)           |

Atuin already keeps a lot out without any setup; see [what's kept out by default](#whats-kept-out-by-default). Filters apply to what Atuin records from then on; to remove what it already has, see [cleaning up](#cleaning-up-what-you-already-recorded).

## One command, with a leading space

Most shells support "ignorespace": a command typed with a leading space isn't saved to history. Atuin honors this convention, and it's the quickest way to keep a single command out. Its output isn't captured either.

```
 echo "this won't be saved"  # note the leading space
```

Bash with bash-preexec

When using bash-preexec (not ble.sh), there's a known issue where ignorespace isn't fully honored. The command won't appear in Atuin, but may still appear in your bash history. See [installation](https://docs.atuin.sh/guide/installation/index.md) for details.

## Commands: `history_filter`

[`history_filter`](https://docs.atuin.sh/configuration/config/#history_filter) keeps any command matching a regular expression out of your history:

```
history_filter = [
    "^ls$",           # exclude bare 'ls', but not 'ls -la'
    "^cd ",           # exclude cd commands
    "--password",     # exclude anything with a password flag
]
```

Patterns are unanchored, so `secret` matches anywhere in the command. Use `^` and `$` when you want to match the whole command exactly.

A filtered command's output isn't captured. When an AI agent runs a matching command, its session keeps only the name of the tool it used, not the command or what it printed.

## A directory: `cwd_filter`

[`cwd_filter`](https://docs.atuin.sh/configuration/config/#cwd_filter) keeps out every command run from a matching directory:

```
cwd_filter = [
    "^/tmp",                    # nothing run from /tmp
    "/node_modules/",           # nothing run inside any node_modules
    "^/home/user/scratch",      # a scratch directory
]
```

These patterns are unanchored regular expressions too, matched against the working directory path.

Nothing run there has its output captured. An AI agent working in a matching directory has every tool call there kept as the tool's name only.

## A command's output: `command_filter`

To keep a command in your history but never store what it prints, use [`command_filter`](https://docs.atuin.sh/configuration/config/#command_filter) in `[output]`:

```
[output]
command_filter = [
    "^terraform plan",
    "^psql ",
]
```

Patterns work like `history_filter`'s: unanchored regular expressions matched against the command.

The filter applies to your AI agents' commands too, even if you don't capture your own output: a matching command is kept in the session, but what it printed isn't.

## A file's contents: `sensitive_files`

Some files hold nothing but credentials: `.env` files, SSH keys, cloud logins. Atuin knows the common ones (see [`sensitive_files`](https://docs.atuin.sh/configuration/config/#sensitive_files) for the list) and keeps their contents out:

- the output of a command that names one, such as `cat .env` or `cat ~/.aws/credentials`
- what an AI agent reads from one
- what an AI agent writes to one, whether with its edit tool or a command like `echo KEY=value > .env`. The session keeps that the agent changed the file, not what it wrote.

Add your own by name or by path:

```
[security]
sensitive_files = [
    "*.secret",                 # any file with this name, anywhere
    "~/work/credentials/**",    # everything under this directory
    "config/master.key",        # this path within any project
]
```

## Your own secrets: `redact_patterns`

Atuin replaces the credentials it recognises with `****` in captured output and AI sessions. If you have secrets in a format it doesn't know, such as internal tokens, give it their pattern:

```
[security]
redact_patterns = [
    "ACME-[0-9A-F]{32}",                    # replace the whole match
    "internal-api-key: (?<secret>\\S+)",    # replace only the `secret` group
]
```

Redaction replaces the value and keeps the text around it. To keep a whole command out instead, use [`history_filter`](#commands-history_filter).

## AI agents' tool calls: `capture_tools`

When Atuin [captures your AI agents' sessions](https://docs.atuin.sh/ai/sessions/index.md), it keeps their tool calls with what each was given and what it returned, after applying everything on this page. To keep only the name of each tool the agent called, turn that off:

```
[ai]
capture_tools = false
```

## Everything from a tool

If a tool spawns interactive shells and you'd rather it recorded nothing at all, guard the `atuin init` call in your shell config:

```
# In .bashrc or .zshrc
if [[ -z "${MY_TOOL_SESSION}" ]]; then
    eval "$(atuin init bash)"
fi
```

Then configure the tool to set `MY_TOOL_SESSION=1` when it spawns a shell. See the [`atuin init` reference](https://docs.atuin.sh/reference/init/index.md) for the other ways to change what the plugin sets up.

Commands from AI agents

You don't need to exclude AI agent commands to keep them out of your way. Atuin tags them with the agent that ran them and hides them from interactive search by default; see [AI Agent Hooks](https://docs.atuin.sh/guide/agent-hooks/index.md).

## What's kept out by default

Without any of the settings above, Atuin:

- **doesn't record commands containing a credential** it recognises, such as AWS, GitHub, OpenAI or Anthropic keys, while [`secrets_filter`](https://docs.atuin.sh/configuration/config/#secrets_filter) is on (the default). It also leaves those commands out of AI sessions.
- **redacts credentials in captured output and AI sessions**, replacing them with `****`: the same formats, plus private keys, passwords in connection strings, and values assigned to names like `DB_PASSWORD` or `api_key`.
- **never stores the output of commands that print a credential**: Atuin's own `atuin key` and `atuin login`, and others such as `gh auth token`, `aws configure get`, `kubectl get secret` and `printenv`. See [`command_filter`](https://docs.atuin.sh/configuration/config/#command_filter) for the list.
- **keeps the contents of credential files out**, as described under [`sensitive_files`](#a-files-contents-sensitive_files).
- **doesn't store output it can't redact quickly**, rather than slow your shell down.

Recognising credentials is best-effort: it only knows common formats. For anything it might miss, use the settings above.

## Cleaning up what you already recorded

Filters only apply going forward. To remove history entries recorded *before* you added a filter, run [`atuin history prune`](https://docs.atuin.sh/reference/prune/index.md):

```
# See what would be removed
atuin history prune --dry-run

# Remove it
atuin history prune
```

This deletes existing entries matching your current `history_filter` and `cwd_filter`. For deleting entries that don't match a filter, see [Deleting History](https://docs.atuin.sh/guide/delete-history/index.md).

AI sessions and captured output already stored keep what they had. Changes to these settings apply to AI sessions once the daemon restarts.
