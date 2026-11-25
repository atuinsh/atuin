# shells: bash zsh dash
# scan: before

x=$(echo a \
 b)
if [ "$XDG_VTNR" = 1 ]; then
  exec sway
fi
echo END
