# shells: bash zsh dash
# scan: before

x=$(if true; then echo a; fi)
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
