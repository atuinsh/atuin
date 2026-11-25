# shells: bash zsh dash
# scan: before

x=$(echo "$(echo ')')")
y=$(echo \))
z=$(echo ')')
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
