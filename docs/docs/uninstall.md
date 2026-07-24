# Uninstalling Atuin

Sorry to see you go!

## If you used the install script (`setup.atuin.sh`)

**Important:** remove shell integration *before* deleting `~/.atuin`. The
installer adds lines that source `~/.atuin/bin/env` (and friends). If that
directory is gone first, login shells that still source those lines can fail —
including graphical sessions that run a login shell (reported with KDE Plasma).

Do the steps in this order:

### 1. Remove PATH helpers added by the binary installer

Search for and delete any line that sources the Atuin env script, for example:

```sh
. "$HOME/.atuin/bin/env"
# or
source "$HOME/.atuin/bin/env"
```

These commonly land in one or more of:

- `~/.profile` (most important for graphical/login shells)
- `~/.bashrc`
- `~/.bash_profile`
- `~/.bash_login`
- `~/.zshrc`
- `~/.zshenv`

Fish users: delete `~/.config/fish/conf.d/atuin.env.fish` if it exists.

A quick way to find leftovers:

```sh
grep -n 'atuin' ~/.profile ~/.bashrc ~/.bash_profile ~/.bash_login \
  "${ZDOTDIR:-$HOME}/.zshrc" "${ZDOTDIR:-$HOME}/.zshenv" 2>/dev/null
```

### 2. Remove shell-plugin init lines

The setup script may append Atuin init to **every** shell config it finds, not
only the shell you use day-to-day. Remove (or comment out) lines containing
`atuin init` from each of these if present:

| Shell | Typical config file | Example line to remove |
| ----- | ------------------- | ---------------------- |
| zsh   | `${ZDOTDIR:-$HOME}/.zshrc` | `eval "$(atuin init zsh)"` |
| bash  | `~/.bashrc` | `eval "$(atuin init bash)"` |
| fish  | `~/.config/fish/config.fish` | `atuin init fish \| source` (often wrapped in `if status is-interactive`) |

Bash installs from the setup script also add bash-preexec. If you no longer need
it for anything else, remove:

- the line `[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh` from
  `~/.bashrc`
- the file `~/.bash-preexec.sh` itself

### 3. Delete Atuin data and binaries

Only after the shell config edits above:

1. Delete the `~/.atuin` directory (binaries + PATH env script)
2. Delete the `~/.config/atuin` directory (config)
3. Delete the `~/.local/share/atuin` directory (history / local DB)

Open a new terminal (or log out and back in) to confirm shells start cleanly.

## Package-manager installs

Otherwise, uninstalling Atuin depends on your system and how you installed it.

For example, on macOS with Homebrew:

```sh
brew uninstall atuin
```

…and then remove the shell integration (`atuin init` lines) from your shell
config as in step 2 above.
