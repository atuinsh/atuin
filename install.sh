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
https://github.com/ellie/atuin

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

LATEST_RELEASE=$(curl -L -s -H 'Accept: application/json' https://github.com/ellie/atuin/releases/latest)
# Allow sed; sometimes it's more readable than ${variable//search/replace}
# shellcheck disable=SC2001
LATEST_VERSION=$(echo "$LATEST_RELEASE" | sed -e 's/.*"tag_name":"\([^"]*\)".*/\1/')

__atuin_install_arch(){
	echo "Arch Linux detected!"
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
		echo "Failed to install atuin! Please try manually: https://aur.archlinux.org/packages/atuin/"
	fi

}

__atuin_install_ubuntu(){
	echo "Ubuntu detected"
	# TODO: select correct AARCH too
	ARTIFACT_URL="https://github.com/ellie/atuin/releases/download/$LATEST_VERSION/atuin_${LATEST_VERSION//v/}_amd64.deb"

	TEMP_DEB="$(mktemp)" &&
	curl -Lo "$TEMP_DEB" "$ARTIFACT_URL"
	if command -v sudo &> /dev/null; then
		sudo dpkg -i "$TEMP_DEB"
	else
		su -l -c "dpkg -i '$TEMP_DEB'"
	fi
	rm -f "$TEMP_DEB"
}

__atuin_install_linux(){
	echo "Detected Linux!"
	echo "Checking distro..."

	if (uname -a | grep -qi "Microsoft"); then
        OS="UbuntuWSL"
    else
        if ! command -v lsb_release &> /dev/null; then
            echo "lsb_release could not be found, unable to determine your distribution"
            echo "If you are using Arch, please get lsb_release from AUR"
            exit 1
        fi
        OS=$(lsb_release -i | awk '{ print $3 }')
    fi

	if [ "$OS" == "Arch" ] || [ "$OS" == "ManjaroLinux" ]; then
		__atuin_install_arch
    elif [ "$OS" == "Ubuntu" ] || [ "$OS" == "Debian" ] || [ "$OS" == "Linuxmint" ] || [ "$OS" == "Parrot" ] || [ "$OS" == "Kali" ] || [ "$OS" == "Elementary" ]; then
		__atuin_install_ubuntu
	else
		# TODO: download a binary or smth
		__atuin_install_unsupported
	fi
}

__atuin_install_mac(){
	echo "Detected Mac!"

	if command -v brew &> /dev/null
	then
		echo "Installing with brew"
		brew tap ellie/atuin
		brew install atuin
	else
		echo "Could not find brew, installing with Cargo"
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
	echo "Unknown or unsupported OS"
	echo "Please check the README at https://github.com/ellie/atuin for manual install instructions"
	echo "If you have any problems, please open an issue!"

	while true; do
		read -r -p "Do you wish to attempt an install with 'cargo'?" yn
		case $yn in
			[Yy]* ) __atuin_install_cargo; break;;
			[Nn]* ) exit;;
			* ) echo "Please answer yes or no.";;
		esac
	done
}

# TODO: would be great to support others!
case "$OSTYPE" in
  linux*)   __atuin_install_linux ;;
  darwin*)  __atuin_install_mac ;;
  msys*)    __atuin_install_unsupported ;;
  solaris*) __atuin_install_unsupported ;;
  bsd*)     __atuin_install_unsupported ;;
  *)        __atuin_install_unsupported ;;
esac

# TODO: Check which shell is in use
# Use of single quotes around $() is intentional here
# shellcheck disable=SC2016
printf '\neval "$(atuin init zsh)"\n' >> ~/.zshrc

curl https://raw.githubusercontent.com/rcaloras/bash-preexec/master/bash-preexec.sh -o ~/.bash-preexec.sh
printf '\n[[ -f ~/.bash-preexec.sh ]] && source ~/.bash-preexec.sh\n' >> ~/.bashrc
# Use of single quotes around $() is intentional here
# shellcheck disable=SC2016
echo 'eval "$(atuin init bash)"' >> ~/.bashrc
