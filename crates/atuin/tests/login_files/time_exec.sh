# shells: bash zsh dash
# scan: before

if [ "$XDG_VTNR" = 1 ]; then exec sway; fi
echo END
