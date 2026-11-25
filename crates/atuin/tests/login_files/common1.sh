# shells: bash zsh dash
# scan: before

eval "$(echo export A=1; echo "# )")"
if [ -z "$DISPLAY" ] && [ "$(cat /dev/null 2>/dev/null || echo -1)" = "" ]; then
  : $(( 1 << 2 ))
fi
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
