# shells: bash zsh dash
# scan: before

case "$XDG_VTNR" in
  # tty one
  1) exec sway ;;
  *) ;;
esac
echo END
