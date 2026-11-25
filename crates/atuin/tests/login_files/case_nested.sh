# shells: bash zsh dash
# scan: before

case "$XDG_VTNR" in
  1) case "$TERM" in linux) exec sway ;; esac ;;
esac
echo END
