# shells: bash zsh
# scan: before

while read -r l; do export "$l"; done < <(echo A=1)
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
