# shells: bash zsh dash
# scan: before

case "$(echo $XDG_VTNR)" in
  1) exec sway ;;
esac
echo END
