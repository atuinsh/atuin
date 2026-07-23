# Shell Integration and Interoperability

Atuin uses shell hooks to capture your command history. This page explains how the integration works, and why Atuin might not record commands in certain environments.

To keep specific commands *out* of your history on purpose, see [Excluding Commands from History](https://docs.atuin.sh/guide/excluding-commands/index.md).

## How Atuin's Shell Integration Works

When you add `eval "$(atuin init <shell>)"` to your shell configuration, Atuin installs hooks that run at specific points in your shell's command lifecycle:

1. **Preexec hook**: Runs *before* each command executes. Atuin records the command text, timestamp, and working directory.
1. **Precmd hook**: Runs *after* each command completes. Atuin records the exit code and duration.

These hooks only activate under specific conditions:

- The shell must be **interactive** (started with `-i` or inherently interactive)
- Your shell configuration file must be **sourced** (`.bashrc`, `.zshrc`, etc.)
- The `atuin init` command must run during shell startup

If any of these conditions aren't met, Atuin's hooks won't be installed, and commands won't be recorded.

### Environment Variables

When Atuin initializes, it sets several environment variables:

| Variable               | Purpose                                                                     |
| ---------------------- | --------------------------------------------------------------------------- |
| `ATUIN_SESSION`        | Unique identifier for this shell session                                    |
| `ATUIN_SHLVL`          | Tracks shell nesting level                                                  |
| `ATUIN_HISTORY_ID`     | Temporary ID for the currently executing command                            |
| `ATUIN_HISTORY_AUTHOR` | Optional command author identity (for example `ellie`, `claude`, `copilot`) |
| `ATUIN_HISTORY_INTENT` | Optional command intent/rationale text                                      |

These variables are used internally to track command execution and associate commands with sessions. If `ATUIN_HISTORY_AUTHOR` is not set, Atuin defaults to the local shell username.

## Embedded Terminals and IDE Integrations

Many development tools include embedded terminals:

- **IDEs**: PyCharm, IntelliJ, VS Code, Cursor, Zed
- **AI coding assistants**: Claude Code, GitHub Copilot CLI, Aider
- **Container environments**: Docker, Podman, devcontainers

These tools often spawn shells differently than your regular terminal, which can prevent Atuin from working.

### Why Atuin Might Not Work

Embedded terminals commonly:

1. **Start non-interactive shells**: Many tools run commands via `bash -c "command"` or similar, which doesn't trigger shell configuration
1. **Skip shell configuration**: Some tools explicitly avoid sourcing `.bashrc`/`.zshrc` for performance or isolation
1. **Use different shell paths**: The embedded terminal might use a different shell than your default

You can verify whether Atuin is active by running:

```
atuin doctor
```

Look for the `shell.preexec` field in the output. If it shows `none`, Atuin's hooks aren't installed in that shell session. To confirm the shell is interactive at all, check that `echo $-` includes an `i`.

### Enabling Atuin in Embedded Terminals

If you want Atuin to record commands from an embedded terminal, you'll need to ensure it starts an interactive shell that sources your configuration.

#### IDE Terminal Settings

Most IDEs let you customize the shell command used for their integrated terminal:

**PyCharm / IntelliJ:**

1. Go to Settings → Tools → Terminal
1. Change "Shell path" to include the `-i` flag:
1. Linux/macOS: `/bin/bash -i` or `/bin/zsh -i`
1. Or create a wrapper script (see below)

**VS Code:**

Add to your `settings.json` (substitute the shells for whatever you use):

```
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

```
#!/bin/bash
# Save as ~/bin/interactive-bash.sh and chmod +x
exec /bin/bash -i "$@"
```

Then configure your IDE to use `~/bin/interactive-bash.sh` as the shell path.

#### Verifying the Fix

After configuring, open a new terminal in your IDE and run:

```
atuin doctor | grep preexec
```

You should see `built-in`, `bash-preexec`, `blesh`, or similar—not `none`.

## Shell-Specific Notes

### Bash

Atuin supports two preexec backends for Bash:

- **ble.sh** (recommended): Full-featured line editor with accurate timing and proper ignorespace support
- **bash-preexec**: Simpler but has some limitations with subshells and ignorespace

The shell integration explicitly checks for interactive mode:

```
if [[ $- != *i* ]]; then
    # Not interactive, skip initialization
    return
fi
```

### Zsh

Zsh has native hook support via `add-zsh-hook`. The integration is straightforward and works reliably in interactive sessions.

### Fish

Fish uses its event system (`fish_preexec` and `fish_postexec` events). It also respects Fish's private mode—commands run with `fish --private` aren't recorded.

### Nushell, xonsh, and PowerShell

These shells are supported too; see [installation](https://docs.atuin.sh/guide/installation/#installing-the-shell-plugin) for how to load the plugin in each.
