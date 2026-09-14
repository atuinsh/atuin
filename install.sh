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
  ATUIN_NON_INTERACTIVE=yes
  if { exec 3</dev/tty; } 2>/dev/null; then
    if [ -t 3 ]; then
        ATUIN_NON_INTERACTIVE=no
    fi
    exec 3<&-
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
  install_script=$(curl --proto '=https' --tlsv1.2 -LsSf https://github.com/atuinsh/atuin/releases/latest/download/atuin-installer.sh)
  echo "$install_script" | sh
}

if ! command -v curl > /dev/null; then
    echo "curl not installed. Please install curl."
    exit
fi

__atuin_install_binary

ATUIN_BIN="$HOME/.atuin/bin/atuin"

echo ""
echo "Atuin installed successfully!"
echo ""

if [ "$ATUIN_NON_INTERACTIVE" != "yes" ]; then
  if ! "$ATUIN_BIN" setup </dev/tty; then
    echo ""
    echo "Setup did not complete. You can run 'atuin setup' any time to finish."
  fi
else
  if ! "$ATUIN_BIN" setup; then
    echo ""
    echo "Setup did not complete. Run 'atuin setup' in a terminal later to configure capture, search, and sync."
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
