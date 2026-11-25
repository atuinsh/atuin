# shells: bash zsh
# scan: before

x=$(echo $'\')')
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
