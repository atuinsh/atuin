# shells: bash zsh dash
# scan: before

case "$XDG_VTNR" in
  2)
    ;;
  1)
    exec sway
    ;;
esac
echo END
