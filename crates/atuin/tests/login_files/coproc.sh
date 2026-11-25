# shells: bash zsh
# scan: unknown

coproc true
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
