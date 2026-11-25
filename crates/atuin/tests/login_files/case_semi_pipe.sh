# shells: zsh
# scan: before

case "$XDG_VTNR" in
  1) export A=1 ;|
  1) exec sway ;;
esac
echo END
