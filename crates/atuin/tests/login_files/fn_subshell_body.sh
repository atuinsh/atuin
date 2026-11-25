# shells: bash zsh dash
# scan: before

f() (
  exec sway
)
[ "$XDG_VTNR" = 1 ] && f
echo END
