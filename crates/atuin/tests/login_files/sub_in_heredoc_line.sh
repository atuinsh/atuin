# shells: bash zsh dash
# scan: before

cat >/dev/null <<EOF
$(case a in a) echo;; esac)
EOF
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
