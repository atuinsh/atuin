# shells: bash zsh dash
# scan: before

# ~/.profile: executed by the command interpreter for login shells.
# the default umask is set in /etc/profile; for setting the umask
# for ssh logins, install and configure the libpam-umask package.
#umask 022

# if running bash
if [ -n "$BASH_VERSION" ]; then
    # include .bashrc if it exists
    if [ -f "$HOME/.bashrc_nonexist" ]; then
	. "$HOME/.bashrc_nonexist"
    fi
fi

# set PATH so it includes user's private bin if it exists
if [ -d "$HOME/bin" ] ; then
    PATH="$HOME/bin:$PATH"
fi
# >>> conda initialize >>>
__conda_setup="$('/nonexistent/conda' 'shell.bash' 'hook' 2> /dev/null)"
if [ $? -eq 0 ]; then
    eval "$__conda_setup"
else
    if [ -f "/nonexistent/etc/profile.d/conda.sh" ]; then
        . "/nonexistent/etc/profile.d/conda.sh"
    else
        export PATH="/nonexistent/bin:$PATH"
    fi
fi
unset __conda_setup
# <<< conda initialize <<<
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"  # This loads nvm
command -v pyenv >/dev/null && eval "$(pyenv init -)"
export GPG_TTY=$(tty)
if [ -z "${DISPLAY}" ] && [ "${XDG_VTNR}" -eq 1 ]; then
  exec sway
fi
echo END
