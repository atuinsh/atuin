# shells: bash zsh
# scan: before

x=$((echo a) | tr a b)
if [ "$XDG_VTNR" = 1 ]; then
  exec sway
fi
echo END
