# shells: bash zsh dash
# scan: before

case "$XDG_VTNR" in
  2) echo esac ;;
  1) exec sway ;;
esac
echo END
