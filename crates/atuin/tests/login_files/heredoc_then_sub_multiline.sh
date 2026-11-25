# shells: bash zsh dash
# scan: before

cat <<EOF >/dev/null; x=$(
echo a
)
body
EOF
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
