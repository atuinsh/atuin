# shells: zsh
# scan: unknown

case $TERM {
  (linux) [ "$XDG_VTNR" = 1 ] && exec sway ;;
}
echo END
