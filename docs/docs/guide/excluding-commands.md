# Excluding Commands from History

Sometimes you do not want a command in your history and Atuin gives you four ways
to exclude the commands.

## Prefix with a space

Most shells support "ignorespace": a command typed with a leading space is not
saved to history. Atuin honors this convention, and it is the quickest way to
keep a single command out.

```shell
 echo "this won't be saved"  # note the leading space
```

!!! warning "Bash with bash-preexec"
    When using bash-preexec (not ble.sh), there is a known issue where
    ignorespace is not fully honored. The command will not appear in Atuin, but may
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
    You do not need to exclude AI agent commands to keep them out of your way.
    Atuin tags them with the agent that ran them and hides them from interactive
    search by default — see [AI Agent Hooks](agent-hooks.md).

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
`cwd_filter`. For deleting entries that do not match a filter, see [Deleting
History](delete-history.md).

## Secrets are filtered automatically

Atuin also refuses to record commands that appear to contain credentials, whatever your
own filters allow. Examples are AWS keys, GitHub and npm tokens, Slack webhooks, and
Stripe keys. This is on by default. For the full list,
see [`secrets_filter`](../configuration/config.md#secrets_filter).
