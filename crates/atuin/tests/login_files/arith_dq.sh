# shells: bash zsh dash
# scan: before

x="$((1+2))"
y=$(( (1) + 2 ))
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
