#! /usr/bin/env bash

set -euo pipefail

LATEST_RELEASE=$(curl -L -s -H 'Accept: application/json' https://github.com/ellie/atuin/releases/latest)
# Allow sed; sometimes it's more readable than ${variable//search/replace}
# shellcheck disable=SC2001
LATEST_VERSION=$(echo "$LATEST_RELEASE" | sed -e 's/.*"tag_name":"\([^"]*\)".*/\1/')


#####################################################################
######################  HELPER FUNCTIONS  ###########################
#####################################################################

check_command() {
  if ! command -v "$1" &>/dev/null; then
    echo "$1 not found. This tool is necessary for this script to work properly."
    echo "Please install it using your package manager and rerun this script."
    exit 1
  fi
}

__atuin_install_arch(){
	echo "Arch Linux detected!"

	if command -v pacman &> /dev/null
	then
		echo "Installing with pacman"
		sudo pacman -S atuin
	else
		echo "Attempting AUR install"
		if command -v yaourt &> /dev/null; then
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
	echo "Ubuntu detected"
	# TODO: select correct AARCH too
	ARTIFACT_URL="https://github.com/ellie/atuin/releases/download/$LATEST_VERSION/atuin_${LATEST_VERSION//v/}_amd64.deb"
	TEMP_DEB="$(mktemp)".deb &&
  curl -Lo "$TEMP_DEB" "$ARTIFACT_URL"
	if command -v sudo &> /dev/null; then
		sudo apt install "$TEMP_DEB"
	else
		su -l -c "apt install '$TEMP_DEB'"
	fi
	rm -f "$TEMP_DEB"
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
	echo "$OS"
	echo "Unknown or unsupported OS"
	echo "Please check the README at https://github.com/ellie/atuin for manual install instructions"
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
__atuin_check_gcc(){
    OS=$(__get_os)
    if command -v gcc &> /dev/null; then
        echo "gcc is installed"
    else
        echo "gcc not installed. Please install gcc."
        exit
    fi
}

__get_os() {
  if [[ "$OSTYPE" == "darwin"* ]]; then
    OS="darwin"
  elif [ -f "/etc/os-release" ]; then
    OS="$(grep -Po '(?<=^ID=).*$' /etc/os-release | tr '[:upper:]' '[:lower:]')" 2>/dev/null
  elif [ -f "/etc/debian_version" ]; then
    OS="debian"
  elif [ -f "/etc/centos-release" ]; then
    OS="centos"
  elif [ -f "/etc/manjaro-release" ]; then
    OS="manjarolinux"
  elif [ -f "/etc/fedora-release" ]; then
    OS="fedora"
    __atuin_check_gcc
  elif [ -f "/etc/arch-release" ]; then
    OS="arch"
  elif [ -f "/etc/redhat-release" ]; then
    OS="redhat"
    __atuin_check_gcc
  elif [ -f "/etc/gentoo-release" ]; then
    OS="gentoo"
  else
    if uname -r | grep -qi "microsoft"; then
      if [ -f "/etc/os-release" ]; then
        PRETTY_NAME="$(grep -Po '(?<=^PRETTY_NAME=).*$' /etc/os-release | tr -d '"' | tr '[:upper:]' '[:lower:]')"
        case "$PRETTY_NAME" in
          "ubuntu"* ) OS="ubuntu" ;;
          "debian"* ) OS="debian" ;;
          "fedora"* ) OS="fedora" ;;
          "arch"*  ) OS="arch"    ;;
          * ) OS="unknownwsl" ;;
        esac
      else
        OS="unknownwsl"
        echo "$OS"
      fi
    else
      echo "$OS"
      echo "Unknown"
      exit 1
    fi
  fi
}

__atuin_install() {
  __get_os
  echo "Detected OS: $OS"
  case "$OSTYPE" in
    linux-android*)
      __atuin_install_termux
      ;;
    linux*)
      case "$OS" in
        "arch"|"manjarolinux"|"endeavouros"|"artix"|"manjaro")
          __atuin_install_arch
          ;;
        "ubuntu"|"ubuntuwsl"|"debian"|"linuxmint"|"parrot"|"kali"|"elementary"|"pop")
          __atuin_install_ubuntu
          ;;
        *)
          __atuin_install_unsupported
          ;;
      esac
      ;;
    darwin*)
      __atuin_install_mac
      ;;
    msys*)
      __atuin_install_unsupported
      ;;
    *)
      __atuin_install_unsupported
      ;;
  esac
}

__print_intro() {
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
https://github.com/ellie/atuin 
☝️Please file an issue if you encounter any problems!
===============================================================================
EOF
}

__print_outro() {
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
Otherwise, Atuin is a hobby project - if you find it valuable, you can help us out!
- ⭐️ Give us a star on GitHub (https://github.com/ellie/atuin)
- 🚀 Contribute! We would love more regular contributors (https://github.com/ellie/atuin)
- 🤑 Sponsor me! If you value the project + want to help keep the hosted sync server free (https://github.com/sponsors/ellie)
~ Ellie 🐢💖
EOF
}

#####################################################################
##########################  SCRIPT START ############################
#####################################################################

check_command curl
check_command sed

__print_intro

# TODO: would be great to support others!
echo "$OSTYPE"
case "$OSTYPE" in
  linux-android*) __atuin_install_termux ;;
  linux*)         __atuin_install ;;
  darwin*)        __atuin_install_mac ;;
  msys*)          __atuin_install_unsupported ;;
  solaris*)       __atuin_install_unsupported ;;
  bsd*)           __atuin_install_unsupported ;;
  *)              __atuin_install_unsupported ;;
esac

case "$SHELL" in
    *bash*)
        BASHRC_PATH="$HOME/.bashrc"
        BASH_PREEXEC_PATH="$HOME/.bash-preexec.sh"

        # Check if .bashrc exists and append the necessary lines
        if [ -f "$BASHRC_PATH" ]; then
            if ! grep -q 'eval "$(atuin init zsh)"' "$BASHRC_PATH"; then
                printf '\neval "$(atuin init zsh)"\n' >> "$BASHRC_PATH"
            fi
            curl https://raw.githubusercontent.com/rcaloras/bash-preexec/master/bash-preexec.sh -o "$BASH_PREEXEC_PATH"
            if ! grep -q '\[\[ -f ~/.bash-preexec.sh \]\] && source ~/.bash-preexec.sh' "$BASHRC_PATH"; then
                printf '\n[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh\n' >> "$BASHRC_PATH"
            fi
            if ! grep -q 'eval "$(atuin init bash)"' "$BASHRC_PATH"; then
                echo 'eval "$(atuin init bash)"' >> "$BASHRC_PATH"
            fi
        else
            echo ".bashrc not found at $BASHRC_PATH"
        fi
        ;;
    *zsh*)
        ZSHRC_PATH="${ZDOTDIR:-$HOME}/.zshrc"
        ZSHRC_XDG_PATH="$HOME/.config/zsh/.zshrc"
      
        if [ -f "$ZSHRC_PATH" ] && ! grep -q 'eval "$(atuin init zsh)"' "$ZSHRC_PATH"; then
            printf '\neval "$(atuin init zsh)"\n' >> "$ZSHRC_PATH"
            echo "Running under zsh (traditional location)"
        elif [ -f "$ZSHRC_XDG_PATH" ] && ! grep -q 'eval "$(atuin init zsh)"' "$ZSHRC_XDG_PATH"; then
            printf '\neval "$(atuin init zsh)"\n' >> "$ZSHRC_XDG_PATH"
            echo "Running under zsh (XDG location)"
        else
            echo ".zshrc not found at $ZSHRC_PATH or $ZSHRC_XDG_PATH"
        fi
        ;;
    *)
        echo "Unknown shell"

        BASHRC_PATH="$HOME/.bashrc"
        ZSHRC_PATH="$HOME/.zshrc"
        ZSHRC_XDG_PATH="$HOME/.config/zsh/.zshrc"
        BASH_PREEXEC_PATH="$HOME/.bash-preexec.sh"

        # Append to .zshrc in traditional location
        if [ -f "$ZSHRC_PATH" ] && ! grep -q 'eval "$(atuin init zsh)"' "$ZSHRC_PATH"; then
            printf '\neval "$(atuin init zsh)"\n' >> "$ZSHRC_PATH"
        # Append to .zshrc in XDG location
        elif [ -f "$ZSHRC_XDG_PATH" ] && ! grep -q 'eval "$(atuin init zsh)"' "$ZSHRC_XDG_PATH"; then
            printf '\neval "$(atuin init zsh)"\n' >> "$ZSHRC_XDG_PATH"
        else
            echo ".zshrc not found at $ZSHRC_PATH or $ZSHRC_XDG_PATH"
        fi

        # Append to .bashrc and download bash-preexec.sh
        if [ -f "$BASHRC_PATH" ]; then
            if ! grep -q 'eval "$(atuin init zsh)"' "$BASHRC_PATH"; then
                printf '\neval "$(atuin init zsh)"\n' >> "$BASHRC_PATH"
            fi
            curl https://raw.githubusercontent.com/rcaloras/bash-preexec/master/bash-preexec.sh -o "$BASH_PREEXEC_PATH"
            if ! grep -q '\[\[ -f ~/.bash-preexec.sh \]\] && source ~/.bash-preexec.sh' "$BASHRC_PATH"; then
                printf '\n[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh\n' >> "$BASHRC_PATH"
            fi
            if ! grep -q 'eval "$(atuin init bash)"' "$BASHRC_PATH"; then
                echo 'eval "$(atuin init bash)"' >> "$BASHRC_PATH"
            fi
        else
            echo ".bashrc not found at $BASHRC_PATH"
        fi
        ;;
esac



__print_outro
