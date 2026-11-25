# shells: bash zsh dash
# scan: before

x=`echo $(echo a)`
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
