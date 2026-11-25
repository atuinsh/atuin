# shells: bash zsh
# scan: before

x=$( [[ $XDG_VTNR == @(1|2) ]] && echo a )
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
