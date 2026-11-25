# shells: bash zsh dash
# scan: before

x="$(cat <<EOF
)
EOF
)"
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
