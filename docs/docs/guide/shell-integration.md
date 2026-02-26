# Shell Integration and Interoperability

Atuin uses shell hooks to capture your command history. This page explains how the integration works, why Atuin might not record commands in certain environments, and how to control what gets recorded.

## How Atuin's Shell Integration Works

When you add `eval "$(atuin init <shell>)"` to your shell configuration, Atuin installs hooks that run at specific points in your shell's command lifecycle:

1. **Preexec hook**: Runs *before* each command executes. Atuin records the command text, timestamp, and working directory.
2. **Precmd hook**: Runs *after* each command completes. Atuin records the exit code and duration.

These hooks only activate under specific conditions:

- The shell must be **interactive** (started with `-i` or inherently interactive)
- Your shell configuration file must be **sourced** (`.bashrc`, `.zshrc`, etc.)
- The `atuin init` command must run during shell startup

If any of these conditions aren't met, Atuin's hooks won't be installed, and commands won't be recorded.

### Environment Variables

When Atuin initializes, it sets several environment variables:

| Variable | Purpose |
|----------|---------|
| `ATUIN_SESSION` | Unique identifier for this shell session |
| `ATUIN_SHLVL` | Tracks shell nesting level |
| `ATUIN_HISTORY_ID` | Temporary ID for the currently executing command |
| `ATUIN_HISTORY_AUTHOR` | Optional command author identity (for example `ellie`, `claude`, `copilot`) |
| `ATUIN_HISTORY_INTENT` | Optional command intent/rationale text |

These variables are used internally to track command execution and associate commands with sessions.
If `ATUIN_HISTORY_AUTHOR` is not set, Atuin defaults to the local shell username.

## Embedded Terminals and IDE Integrations

Many development tools include embedded terminals:

- **IDEs**: PyCharm, IntelliJ, VS Code, Cursor, Zed
- **AI coding assistants**: Claude Code, GitHub Copilot CLI, Aider
- **Container environments**: Docker, Podman, devcontainers

These tools often spawn shells differently than your regular terminal, which can prevent Atuin from working.

### Why Atuin Might Not Work

Embedded terminals commonly:

1. **Start non-interactive shells**: Many tools run commands via `bash -c "command"` or similar, which doesn't trigger shell configuration
2. **Skip shell configuration**: Some tools explicitly avoid sourcing `.bashrc`/`.zshrc` for performance or isolation
3. **Use different shell paths**: The embedded terminal might use a different shell than your default

You can verify whether Atuin is active by running:

```shell
atuin doctor
```

Look for the `shell.preexec` field in the output. If it shows `none`, Atuin's hooks aren't installed in that shell session.

### Enabling Atuin in Embedded Terminals

If you want Atuin to record commands from an embedded terminal, you'll need to ensure it starts an interactive shell that sources your configuration.

#### IDE Terminal Settings

Most IDEs let you customize the shell command used for their integrated terminal:

**PyCharm / IntelliJ:**

1. Go to Settings → Tools → Terminal
2. Change "Shell path" to include the `-i` flag:
   - Linux/macOS: `/bin/bash -i` or `/bin/zsh -i`
   - Or create a wrapper script (see below)

**VS Code:**

Add to your `settings.json` (substitute the shells for whatever you use):

```json
{
  "terminal.integrated.profiles.linux": {
    "bash": {
      "path": "/bin/bash",
      "args": ["-i"]
    }
  },
  "terminal.integrated.profiles.osx": {
    "zsh": {
      "path": "/bin/zsh",
      "args": ["-i"]
    }
  }
}
```

#### Wrapper Script Approach

For tools that don't easily support shell arguments, create a wrapper script:

```shell
#!/bin/bash
# Save as ~/bin/interactive-bash.sh and chmod +x
exec /bin/bash -i "$@"
```

Then configure your IDE to use `~/bin/interactive-bash.sh` as the shell path.

#### Verifying the Fix

After configuring, open a new terminal in your IDE and run:

```shell
atuin doctor | grep preexec
```

You should see `built-in`, `bash-preexec`, `blesh`, or similar—not `none`.

## Excluding Commands from History

Sometimes you *don't* want certain commands in your history. This is common when:

- AI coding tools run many automated commands (git status, file listings, etc.)
- You're running sensitive commands you don't want synced
- Build tools or scripts generate repetitive command noise

### Using history_filter

The `history_filter` option in `~/.config/atuin/config.toml` lets you exclude commands matching specific patterns:

```toml
history_filter = [
    "^ls$",           # Exclude bare 'ls' commands
    "^cd ",           # Exclude cd commands
    "^cat ",          # Exclude cat commands
]
```

Patterns are regular expressions. They're unanchored by default, so `secret` matches anywhere in a command. Use `^` and `$` for exact matching.

### Using cwd_filter

To exclude all commands run from specific directories:

```toml
cwd_filter = [
    "^/tmp",                    # Exclude commands run from /tmp
    "/node_modules/",           # Exclude commands from any node_modules
    "^/home/user/scratch",      # Exclude a scratch directory
]
```

### Prefix with Space (ignorespace)

Most shells support "ignorespace"—commands prefixed with a space aren't saved to history. Atuin honors this convention:

```shell
 echo "this won't be saved"  # Note the leading space
```

!!! warning "Bash with bash-preexec"
    When using bash-preexec (not ble.sh), there's a known issue where ignorespace isn't fully honored. The command won't appear in Atuin, but may still appear in your bash history. See [installation](installation.md) for details.

### Disabling Atuin for Specific Tools

If a tool spawns interactive shells but you don't want its commands recorded, you have several options:

#### Option 1: Environment Variable Check

Modify your shell configuration to skip Atuin initialization based on an environment variable:

```shell
# In .bashrc or .zshrc
if [[ -z "${MY_TOOL_SESSION}" ]]; then
    eval "$(atuin init bash)"
fi
```

Then configure your tool to set `MY_TOOL_SESSION=1` when spawning shells.

#### Option 2: Use history_filter for Tool-Specific Patterns

If the tool runs predictable commands, filter them:

```toml
history_filter = [
    "^git status$",
    "^git diff",
    "^ls -la$",
]
```

#### Option 3: Filter by Directory

If the tool operates in specific directories:

```toml
cwd_filter = [
    "^/path/to/tool/workspace",
]
```

### Cleaning Up Existing History

After adding filters, you can remove matching entries from your existing history:

```shell
atuin history prune
```

This removes entries that match your current `history_filter` or `cwd_filter` patterns.

## Shell-Specific Notes

### Bash

Atuin supports two preexec backends for Bash:

- **ble.sh** (recommended): Full-featured line editor with accurate timing and proper ignorespace support
- **bash-preexec**: Simpler but has some limitations with subshells and ignorespace

The shell integration explicitly checks for interactive mode:

```bash
if [[ $- != *i* ]]; then
    # Not interactive, skip initialization
    return
fi
```

### Zsh

Zsh has native hook support via `add-zsh-hook`. The integration is straightforward and works reliably in interactive sessions.

### Fish

Fish uses its event system (`fish_preexec` and `fish_postexec` events). It also respects Fish's private mode—commands run with `fish --private` aren't recorded.

## Troubleshooting

### Commands aren't being recorded

1. Run `atuin doctor` and check the output
2. Verify `shell.preexec` is not `none`
3. Ensure your shell is interactive (`echo $-` should contain `i`)
4. Check that `atuin init` is in your shell config and being sourced

### Commands from a specific tool aren't recorded

1. Check if the tool starts an interactive shell
2. Try configuring the tool to use `bash -i` or `zsh -i`
3. Use a wrapper script if the tool doesn't support shell arguments

### Too many commands are being recorded

1. Add patterns to `history_filter` in your config
2. Use `cwd_filter` for directory-based exclusion
3. Prefix sensitive commands with a space
4. Run `atuin history prune` to clean existing entries

### Atuin works in terminal but not in IDE

This is the most common issue. The IDE's embedded terminal likely isn't starting an interactive shell. See [Enabling Atuin in Embedded Terminals](#enabling-atuin-in-embedded-terminals) above.
