# shells: bash dash
# scan: unknown

x=${PWD%(}
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
