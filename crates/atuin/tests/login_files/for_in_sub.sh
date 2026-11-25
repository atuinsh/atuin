# shells: bash zsh dash
# scan: before

x=$(for i in 1 2; do echo $i; done)
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
