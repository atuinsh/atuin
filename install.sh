#! /usr/bin/env bash

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
# For binary install, use the first argument as the installation directory.
INSTALL_DIR=${INSTALL_DIR:-/usr/local/bin}

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

__atuin_install_ubuntu(){
	if [ "$(dpkg --print-architecture)" = "amd64" ]; then
		echo "Ubuntu detected"
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
		echo "Ubuntu detected, but not amd64"
		__atuin_install_binary
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
		"ubuntu" | "ubuntuwsl" | "debian" | "linuxmint" | "parrot" | "kali" | "elementary" | "pop")
			__atuin_install_ubuntu;;
		*)
			__atuin_install_binary;;
	esac
}

__atuin_install_mac(){
	echo "Detected Mac!"

	if command -v brew &> /dev/null
	then
		echo "Installing with brew"
		brew install atuin
	else
		echo "Could not find brew, installing binary"
		__atuin_install_binary
	fi

}

__atuin_install_termux(){
	echo "Termux detected!"

	if command -v pkg &> /dev/null; then
		echo "Installing with pkg"
		pkg install atuin
	else
		echo "Could not find pkg"
		__atuin_install_binary
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

__atuin_install_binary() {
    local target
    local arch

    arch=$(uname -sm)

    case $arch in
    "Linux x86_64")
        target="x86_64-unknown-linux-musl"
        ;;
    "Linux aarch64")
        target="aarch64-unknown-linux-gnu"
        ;;
    "Darwin x86_64")
        target="x86_64-apple-darwin"
        ;;
    "Darwin arm64")
        target="aarch64-apple-darwin"
        ;;
    *)
        __atuin_install_unsupported
        return
        ;;
    esac

    local latest_version
    local archive_name
    local binary_url
    local download_dir
    local dir_usr

    echo "Will copy atuin binary to $INSTALL_DIR..."
    if [[ ! -v ATUIN_NO_PROMPT ]]; then
        read -p "Okay? [Y/n]: " -n 1 -r
        echo

        if [[ ! $REPLY =~ ^[Y]$ ]]
        then
            exit
        fi
    fi

    latest_version=$(curl -s https://api.github.com/repos/atuinsh/atuin/releases/latest | grep tag_name  | grep -Eo "v[0-9]+\.[0-9]+\.[0-9]+")
    archive_name=atuin-$latest_version-$target
    binary_url="https://github.com/atuinsh/atuin/releases/download/$latest_version/$archive_name.tar.gz"
    download_dir=$(mktemp -d)
    echo "Downloading atuin $latest_version from the latest github release..."
    curl -sL "$binary_url" | tar -xz -C "$download_dir" -f -

    if [[ "$OSTYPE" == darwin* ]]; then
        dir_usr=$(stat -f "%Su" "$INSTALL_DIR")
    else
        dir_usr=$(stat -c "%U" "$INSTALL_DIR")
    fi

    echo "Copying atuin binary to $INSTALL_DIR ..."
    sudo -u "$dir_usr" mkdir -p "$INSTALL_DIR"
    sudo -u "$dir_usr" mv "$download_dir/$archive_name/atuin" "$INSTALL_DIR/"
    rm -r "$download_dir"
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
if ! grep -q "atuin init zsh" ~/.zshrc; then
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

If you have any issues, please open an issue on GitHub or visit our Discord (https://discord.gg/dPhv2B3x)!

If you love Atuin, please give us a star on GitHub! It really helps ⭐️ https://github.com/atuinsh/atuin

Please run "atuin register" to get setup with sync, or "atuin login" if you already have an account

EOF
