# shells: bash zsh dash
# scan: before

[ "$XDG_VTNR" = 2 ] && exec startx
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
