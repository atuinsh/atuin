#! /bin/sh
set -eu

ATUIN_NON_INTERACTIVE="no"

for arg in "$@"; do
  case "$arg" in
    --non-interactive) ATUIN_NON_INTERACTIVE="yes" ;;
    *) ;;
  esac
done

if [ "$ATUIN_NON_INTERACTIVE" != "yes" ]; then
  if [ -t 0 ] || { true </dev/tty; } 2>/dev/null; then
    ATUIN_NON_INTERACTIVE="no"
  else
    ATUIN_NON_INTERACTIVE="yes"
  fi
fi

cat << EOF
 _______  _______  __   __  ___   __    _
|   _   ||       ||  | |  ||   | |  |  | |
|  |_|  ||_     _||  | |  ||   | |   |_| |
|       |  |   |  |  |_|  ||   | |       |
|       |  |   |  |       ||   | |  _    |
|   _   |  |   |  |       ||   | | | |   |
|__| |__|  |___|  |_______||___| |_|  |__|

Magical shell history

Atuin setup
https://github.com/atuinsh/atuin
https://forum.atuin.sh

Please file an issue or reach out on the forum if you encounter any problems!

===============================================================================

EOF

__atuin_install_binary(){
  curl --proto '=https' --tlsv1.2 -LsSf https://github.com/atuinsh/atuin/releases/latest/download/atuin-installer.sh | sh
}

if ! command -v curl > /dev/null; then
    echo "curl not installed. Please install curl."
    exit
fi


__atuin_install_binary

# TODO: Check which shell is in use
# Use of single quotes around $() is intentional here
# shellcheck disable=SC2016
if ! grep -q "atuin init zsh" "${ZDOTDIR:-$HOME}/.zshrc"; then
  printf '\neval "$(atuin init zsh)"\n' >> "${ZDOTDIR:-$HOME}/.zshrc"
fi

# Use of single quotes around $() is intentional here
# shellcheck disable=SC2016

if ! grep -q "atuin init bash" ~/.bashrc; then
  curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/rcaloras/bash-preexec/master/bash-preexec.sh -o ~/.bash-preexec.sh
  printf '\n[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh\n' >> ~/.bashrc
  echo 'eval "$(atuin init bash)"' >> ~/.bashrc
fi

if [ -f "$HOME/.config/fish/config.fish" ]; then
  if ! grep -q "atuin init fish" "$HOME/.config/fish/config.fish"; then
    printf '\nif status is-interactive\n    atuin init fish | source\nend\n' >> "$HOME/.config/fish/config.fish"
  fi
fi

ATUIN_BIN="$HOME/.atuin/bin/atuin"

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

  printf "Sign up for a sync account? [Y/n] "
  read -r sync_answer </dev/tty || sync_answer="n"
  sync_answer="${sync_answer:-y}"

  case "$sync_answer" in
    [yY]*)
      echo ""
      if ! "$ATUIN_BIN" register </dev/tty; then
        echo ""
        echo "Registration did not complete. You can run 'atuin register' any time to try again."
      fi
      ;;
    *)
      echo ""
      printf "Already have an account? Log in with 'atuin login'.\n"
      echo "You can also run 'atuin register' any time to create one."
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
  "$ATUIN_BIN" setup </dev/tty
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
