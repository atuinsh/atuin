# shells: bash zsh dash
# scan: before

x="${y:-$(case a in a) echo b;; esac)}"
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
