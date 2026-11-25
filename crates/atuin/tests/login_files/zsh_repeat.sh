# shells: zsh
# scan: unknown

repeat 1 { : }
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
