# shells: zsh
# scan: unknown

case $TERM {
  (linux) export A=1 ;;
}
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
