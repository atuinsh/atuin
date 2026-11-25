# shells: bash zsh dash
# scan: before

( case a in a) : ;; esac )
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
