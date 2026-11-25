# shells: bash zsh dash
# scan: before

x=${PWD%)}
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
