# shells: bash zsh dash
# scan: before

x="`echo "a)"`"
if [ "$XDG_VTNR" = 1 ]; then
  exec sway
fi
echo END
