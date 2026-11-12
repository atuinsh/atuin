#! /bin/sh
set -eu

ATUIN_NON_INTERACTIVE="no"

# Everything but --non-interactive is for `atuin setup`, e.g. `sh -s -- --no-up-arrow --sync off`.
for arg in "$@"; do
  shift
  case "$arg" in
    --non-interactive) ATUIN_NON_INTERACTIVE="yes" ;;
    *) set -- "$@" "$arg" ;;
  esac
done

if [ "$ATUIN_NON_INTERACTIVE" != "yes" ]; then
  ATUIN_NON_INTERACTIVE=yes
  # In a subshell: without a terminal, `exec`'s failed redirection would end the script.
  if (exec 3</dev/tty && [ -t 3 ]) 2>/dev/null; then
    ATUIN_NON_INTERACTIVE=no
  fi
fi

if [ -t 1 ]; then
  GREEN=$(printf '\033[32m')
  RESET=$(printf '\033[0m')
  BACK='\r'
else
  GREEN=""
  RESET=""
  BACK='\n'
fi

if ! command -v curl > /dev/null; then
  echo "Atuin needs curl to install. Please install it, then try again."
  exit 1
fi

__atuin_install_binary(){
  install_script=$(curl --proto '=https' --tlsv1.2 -LsSf https://github.com/atuinsh/atuin/releases/latest/download/atuin-installer.sh) || return
  echo "$install_script" | sh
}

ATUIN_BIN="$HOME/.atuin/bin/atuin"
ATUIN_INSTALL_LOG=$(mktemp)
trap 'rm -f "$ATUIN_INSTALL_LOG"' EXIT

printf '\n  Installing Atuin...'
# What the installer says only matters if it fails.
if ! __atuin_install_binary > "$ATUIN_INSTALL_LOG" 2>&1; then
  printf '\n\n'
  cat "$ATUIN_INSTALL_LOG"
  printf '\nAtuin did not install. Please file an issue, or ask on the forum: https://forum.atuin.sh\n'
  exit 1
fi
printf '%b  %s✓%s Installed Atuin %s\n' "$BACK" "$GREEN" "$RESET" "$("$ATUIN_BIN" --version | cut -d' ' -f2)"

# An `atuin setup` that takes flags does everything below itself: hooks, import and sync. This
# script is served from main, but installs the latest release, which may not have it yet.
if "$ATUIN_BIN" setup --help 2>/dev/null | grep -q -- "--no-up-arrow"; then
  if [ "$ATUIN_NON_INTERACTIVE" != "yes" ]; then
    if ! "$ATUIN_BIN" setup "$@" </dev/tty; then
      echo ""
      echo "Setup did not complete. You can run 'atuin setup' any time to finish."
    fi
  # Nobody to sign in, and an import handed to the daemon may not outlive a CI job.
  elif ! "$ATUIN_BIN" setup --yes --no-import "$@"; then
    echo ""
    echo "Setup did not complete. You can run 'atuin setup' any time to finish."
  fi
  exit 0
fi

# TODO: Check which shell is in use
# Use of single quotes around $() is intentional here
# shellcheck disable=SC2016
if ! grep -q "atuin init zsh" "${ZDOTDIR:-$HOME}/.zshrc"; then
  printf '\neval "$(atuin init zsh)"\n' >> "${ZDOTDIR:-$HOME}/.zshrc"
fi

# Use of single quotes around $() is intentional here
# shellcheck disable=SC2016

if ! grep -q "atuin init bash" "$HOME/.bashrc"; then
  echo 'eval "$(atuin init bash)"' >> "$HOME/.bashrc"
fi

if [ -f "$HOME/.config/fish/config.fish" ]; then
  if ! grep -q "atuin init fish" "$HOME/.config/fish/config.fish"; then
    printf '\nif status is-interactive\n    atuin init fish | source\nend\n' >> "$HOME/.config/fish/config.fish"
  fi
fi

__atuin_install_agent_hook(){
  agent="$1"
  agent_name="$2"
  agent_config_dir="$3"
  shift 3

  detected="no"

  if [ -d "$agent_config_dir" ]; then
    detected="yes"
  else
    for agent_command in "$@"; do
      if command -v "$agent_command" > /dev/null 2>&1; then
        detected="yes"
        break
      fi
    done
  fi

  if [ "$detected" = "yes" ]; then
    echo "Detected $agent_name — installing Atuin hooks..."
    if ! "$ATUIN_BIN" hook install "$agent"; then
      echo "Failed to install Atuin hooks for $agent_name (this version of Atuin may not support it yet)."
    fi
    echo ""
  fi
}

__atuin_install_agent_hook "claude-code" "Claude Code" "$HOME/.claude" claude
__atuin_install_agent_hook "codex" "Codex" "$HOME/.codex" codex
__atuin_install_agent_hook "opencode" "opencode" "${XDG_CONFIG_HOME:-$HOME/.config}/opencode" opencode
__atuin_install_agent_hook "pi" "pi" "$HOME/.config/pi" pi

echo ""
echo "Atuin installed successfully!"
echo ""

if [ "$ATUIN_NON_INTERACTIVE" != "yes" ]; then

  printf "Would you like to import your existing shell history into Atuin? [Y/n] "
  read -r import_answer </dev/tty || import_answer="n"
  import_answer="${import_answer:-y}"

  case "$import_answer" in
    [yY]*)
      echo ""
      if ! "$ATUIN_BIN" import auto; then
        echo ""
        echo "History import failed. You can retry later with 'atuin import auto'."
      fi
      echo ""
      ;;
    *)
      echo "Skipping history import. You can always run 'atuin import auto' later."
      echo ""
      ;;
  esac

  cat << EOF
Sync your history across all your machines with Atuin Cloud:

  - End-to-end encrypted — only you can read your data
  - Access your history from any device
  - Never lose your history, even if you wipe a machine

EOF

  echo "Set up sync with an Atuin account?"
  echo ""
  echo "  1) Yes - create a new account"
  echo "  2) Log in to an existing account"
  echo "  3) Skip sync for now"
  echo ""
  printf "Select an option [1/2/3] (default 1): "
  read -r sync_answer </dev/tty || sync_answer="3"
  sync_answer="${sync_answer:-1}"

  case "$sync_answer" in
    1|[yYcC]*)
      echo ""
      if ! "$ATUIN_BIN" register </dev/tty; then
        echo ""
        echo "Registration did not complete. You can run 'atuin register' any time to try again."
      fi
      ;;
    2|[lL]*)
      echo ""
      # Silences pre-#3916 401 log spam; drop once that fix is in the latest release.
      if ! ATUIN_LOG="warn,atuin_client::hub=off" "$ATUIN_BIN" login </dev/tty; then
        echo ""
        echo "Login did not complete. You can run 'atuin login' any time to try again."
      fi
      ;;
    *)
      echo ""
      echo "Skipping sync setup."
      echo "You can run 'atuin register' any time to create an account,"
      echo "or 'atuin login' if you already have one."
      ;;
  esac

else
  echo "Non-interactive environment detected — skipping setup prompts."
  echo "You can run the following commands manually after installation:"
  echo ""
  echo "  atuin import auto       Import your existing shell history"
  echo "  atuin register          Sign up for a sync account"
  echo "  atuin login             Log in to an existing sync account"
fi

if [ "$ATUIN_NON_INTERACTIVE" != "yes" ]; then
  if ! "$ATUIN_BIN" setup </dev/tty; then
    echo ""
    echo "Setup did not complete. You can run 'atuin setup' any time to finish."
  fi
fi

cat << EOF

 _______  __   __  _______  __    _  ___   _    __   __  _______  __   __
|       ||  | |  ||   _   ||  |  | ||   | | |  |  | |  ||       ||  | |  |
|_     _||  |_|  ||  |_|  ||   |_| ||   |_| |  |  |_|  ||   _   ||  | |  |
  |   |  |       ||       ||       ||      _|  |       ||  | |  ||  |_|  |
  |   |  |       ||       ||  _    ||     |_   |_     _||  |_|  ||       |
  |   |  |   _   ||   _   || | |   ||    _  |    |   |  |       ||       |
  |___|  |__| |__||__| |__||_|  |__||___| |_|    |___|  |_______||_______|

Thanks for installing Atuin! I really hope you like it.

If you have any issues, please open an issue on GitHub or visit our forum (https://forum.atuin.sh)!

If you love Atuin, please give us a star on GitHub! It really helps ⭐️ https://github.com/atuinsh/atuin

===============================================================================

 ⚠️  Please restart your shell or open a new terminal for Atuin to take effect!

===============================================================================
EOF
