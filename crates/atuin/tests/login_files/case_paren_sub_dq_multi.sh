# shells: bash zsh dash
# scan: before

msg="$(
  case "$XDG_VTNR" in
    1) echo one;;
  esac
)"
if [ "$XDG_VTNR" = 1 ]; then
  exec sway
fi
echo END
