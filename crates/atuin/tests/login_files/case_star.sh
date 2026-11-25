# shells: bash zsh dash
# scan: before

case "$XDG_VTNR" in
  [2-9]) : ;;
  *) exec sway ;;
esac
echo END
