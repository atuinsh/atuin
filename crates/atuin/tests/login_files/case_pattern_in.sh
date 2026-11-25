# shells: bash zsh dash
# scan: before

case "$XDG_VTNR" in
  in|do) : ;;
  1) exec sway ;;
esac
echo END
