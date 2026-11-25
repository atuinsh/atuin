# shells: bash zsh dash
# scan: before

x=$(
  case "$TERM" in
    linux) echo l ;;
    *) echo o ;;
  esac
)
if [ "$XDG_VTNR" = 1 ]; then
  exec sway
fi
echo END
