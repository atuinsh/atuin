# shells: bash zsh
# scan: before

[[ $TERM =~ (case|linux) ]] && echo
if [ "$XDG_VTNR" = 1 ]; then
  exec sway
fi
echo END
