# shells: zsh
# scan: before

[[ $XDG_VTNR ==
   1 ]] && exec sway
echo END
