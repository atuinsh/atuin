# shells: bash zsh
# scan: before

[[ -z $DISPLAY
   && $XDG_VTNR -eq 1 ]] && exec sway
echo END
