#! /usr/bin/env bash

if [[ "${BASH_VERSION%%.*}" -eq 3 ]]; then
    echo "Atuin has limited support for Bash 3.2. The Atuin config enter_accept cannot be turned off." >&2
    echo "To turn off enter_accept, please upgrade your version of bash (possibly via homebrew or ports)" >&2
fi

set -euo pipefail

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

Please file an issue if you encounter any problems!

===============================================================================

EOF

if ! command -v curl &> /dev/null; then
    echo "curl not installed. Please install curl."
    exit
elif ! command -v sed &> /dev/null; then
    echo "sed not installed. Please install sed."
    exit
fi

LATEST_RELEASE=$(curl -L -s -H 'Accept: application/json' https://github.com/atuinsh/atuin/releases/latest)
# Allow sed; sometimes it's more readable than ${variable//search/replace}
# shellcheck disable=SC2001
LATEST_VERSION=$(echo "$LATEST_RELEASE" | sed -e 's/.*"tag_name":"\([^"]*\)".*/\1/')

__atuin_install_arch(){
	echo "Arch Linux detected!"

	if command -v pacman &> /dev/null
	then
		echo "Installing with pacman"
		sudo pacman -S atuin
	else
		echo "Attempting AUR install"
		if command -v paru &> /dev/null; then
			echo "Found paru"
			paru -S atuin
		elif command -v yaourt &> /dev/null; then
			echo "Found yaourt"
			yaourt -S atuin
		elif command -v yay &> /dev/null; then
			echo "Found yay"
			yay -S atuin
		elif command -v pakku &> /dev/null; then
			echo "Found pakku"
			pakku -S atuin
		elif command -v pamac &> /dev/null; then
			echo "Found pamac"
			pamac install atuin
		else
			echo "Failed to install atuin! Please try manually: https://aur.archlinux.org/packages/atuin-git/"
		fi
	fi

}

__atuin_install_deb_based(){
	if [ "$(dpkg --print-architecture)" = "amd64" ]; then
		echo "Detected distro: $OS"
		ARTIFACT_URL="https://github.com/atuinsh/atuin/releases/download/$LATEST_VERSION/atuin_${LATEST_VERSION//v/}_amd64.deb"
		TEMP_DEB="$(mktemp)".deb &&
		curl -Lo "$TEMP_DEB" "$ARTIFACT_URL"
		if command -v sudo &> /dev/null; then
			sudo apt install "$TEMP_DEB"
		else
			su -l -c "apt install '$TEMP_DEB'"
		fi
		rm -f "$TEMP_DEB"
	else
		echo "$OS detected, but not amd64"
		__atuin_install_unsupported
	fi
}

__atuin_install_linux(){
	echo "Detected Linux!"
	echo "Checking distro..."
	if (uname -a | grep -qi "Microsoft"); then
    OS="ubuntuwsl"
  elif ! command -v lsb_release &> /dev/null; then
    echo "lsb_release could not be found. Falling back to /etc/os-release"
    OS="$(grep -Po '(?<=^ID=).*$' /etc/os-release | tr '[:upper:]' '[:lower:]')" 2>/dev/null
  else
    OS=$(lsb_release -i | awk '{ print $3 }' | tr '[:upper:]' '[:lower:]')
  fi
	case "$OS" in
		"arch" | "manjarolinux" | "endeavouros")
			__atuin_install_arch;;
		"ubuntu" | "ubuntuwsl" | "debian" | "linuxmint" | "parrot" | "kali" | "elementary" | "pop" | "neon")
			__atuin_install_deb_based;;
		*)
			# TODO: download a binary or smth
			__atuin_install_unsupported;;
	esac
}

__atuin_install_mac(){
	echo "Detected Mac!"

	if command -v brew &> /dev/null
	then
		echo "Installing with brew"
		brew install atuin
	else
		echo "Could not find brew, installing with Cargo"
		__atuin_install_unsupported
	fi

}

__atuin_install_termux(){
	echo "Termux detected!"

	if command -v pkg &> /dev/null; then
		echo "Installing with pkg"
		pkg install atuin
	else
		echo "Could not find pkg"
		__atuin_install_unsupported
	fi
}

__atuin_install_cargo(){
	echo "Attempting install with cargo"

	if ! command -v cargo &> /dev/null
	then
		echo "cargo not found! Attempting to install rustup"

		if command -v rustup &> /dev/null
		then
			echo "rustup was found, but cargo wasn't. Something is up with your install"
			exit 1
		fi

		curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -q

		echo "rustup installed! Attempting cargo install"

	fi

	cargo install atuin
}

__atuin_install_unsupported(){
	echo "Unknown or unsupported OS or architecture"
	echo "Please check the README at https://github.com/atuinsh/atuin for manual install instructions"
	echo "If you have any problems, please open an issue!"

	while true; do
		read -r -p "Do you wish to attempt an install with 'cargo'? [Y/N] " yn
		case $yn in
			[Yy]* ) __atuin_install_cargo; break;;
			[Nn]* ) exit;;
			* ) echo "Please answer yes or no.";;
		esac
	done
}

# TODO: would be great to support others!
case "$OSTYPE" in
  linux-android*) __atuin_install_termux ;;
  linux*)         __atuin_install_linux ;;
  darwin*)        __atuin_install_mac ;;
  msys*)          __atuin_install_unsupported ;;
  solaris*)       __atuin_install_unsupported ;;
  bsd*)           __atuin_install_unsupported ;;
  *)              __atuin_install_unsupported ;;
esac

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

If you have any issues, please open an issue on GitHub or visit our Discord (https://discord.gg/jR3tfchVvW)!

If you love Atuin, please give us a star on GitHub! It really helps ⭐️ https://github.com/atuinsh/atuin

Please run "atuin register" to get setup with sync, or "atuin login" if you already have an account

EOF
