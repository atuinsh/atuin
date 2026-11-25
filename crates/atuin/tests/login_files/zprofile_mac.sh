# shells: zsh
# scan: before

eval "$(/nonexistent/brew shellenv 2>/dev/null)"
export PATH="$HOME/.cargo/bin:${PATH}"
typeset -U path PATH
path=($path ~/.local/bin(N))
if (( $+commands[sway] )) && [[ -z $WAYLAND_DISPLAY && $XDG_VTNR == 1 ]]; then
  exec sway
fi
echo END
