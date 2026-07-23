# Excluding Commands from History

Sometimes you don't want a command in your history and Atuin gives you four ways
ways to exclude the commands.

## Prefix with a space

Most shells support "ignorespace": a command typed with a leading space isn't
saved to history. Atuin honors this convention, and it's the quickest way to
keep a single command out.

```shell
 echo "this won't be saved"  # note the leading space
```

!!! warning "Bash with bash-preexec"
    When using bash-preexec (not ble.sh), there's a known issue where
    ignorespace isn't fully honored. The command won't appear in Atuin, but may
    still appear in your bash history. See [installation](installation.md) for
    details.

## Filter by command: `history_filter`

[`history_filter`](../configuration/config.md#history_filter) excludes any
command matching a regular expression:

```toml
history_filter = [
    "^ls$",           # exclude bare 'ls', but not 'ls -la'
    "^cd ",           # exclude cd commands
    "--password",     # exclude anything with a password flag
]
```

Patterns are unanchored, so `secret` matches anywhere in the command. Use `^`
and `$` when you want to match the whole command exactly.

## Filter by directory: `cwd_filter`

[`cwd_filter`](../configuration/config.md#cwd_filter) excludes every command
run from a matching directory:

```toml
cwd_filter = [
    "^/tmp",                    # nothing run from /tmp
    "/node_modules/",           # nothing run inside any node_modules
    "^/home/user/scratch",      # a scratch directory
]
```

These patterns are unanchored regular expressions too, matched against the
working directory path.

## Skip Atuin entirely for a tool

If a tool spawns interactive shells and you'd rather it recorded nothing at
all, guard the `atuin init` call in your shell config:

```shell
# In .bashrc or .zshrc
if [[ -z "${MY_TOOL_SESSION}" ]]; then
    eval "$(atuin init bash)"
fi
```

Then configure the tool to set `MY_TOOL_SESSION=1` when it spawns a shell. See
the [`atuin init` reference](../reference/init.md) for the other ways to change
what the plugin sets up.

!!! tip "Commands from AI agents"
    You don't need to exclude AI agent commands to keep them out of your way.
    Atuin tags them with the agent that ran them and hides them from interactive
    search by default—see [AI Agent Hooks](agent-hooks.md).

## Cleaning up commands you already recorded

Filters only apply going forward. To remove entries recorded *before* you added
a filter, run [`atuin history prune`](../reference/prune.md):

```shell
# See what would be removed
atuin history prune --dry-run

# Remove it
atuin history prune
```

This deletes existing entries matching your current `history_filter` and
`cwd_filter`. For deleting entries that don't match a filter, see [Deleting
History](delete-history.md).

## Secrets are filtered automatically

Independently of your own filters, Atuin refuses to record commands that look
like they contain credentials—AWS keys, GitHub and npm tokens, Slack
webhooks, Stripe keys, and more. This is on by default; see
[`secrets_filter`](../configuration/config.md#secrets_filter) for the full
list.
