#! /bin/sh
set -eu

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
elif ! command -v sed > /dev/null; then
    echo "sed not installed. Please install sed."
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
  curl https://raw.githubusercontent.com/rcaloras/bash-preexec/master/bash-preexec.sh -o ~/.bash-preexec.sh
  printf '\n[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh\n' >> ~/.bashrc
  echo 'eval "$(atuin init bash)"' >> ~/.bashrc
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

Please run "atuin register" to get setup with sync, or "atuin login" if you already have an account

EOF

