# shells: bash zsh dash
# scan: before

echo "exec sway" >/dev/null
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
