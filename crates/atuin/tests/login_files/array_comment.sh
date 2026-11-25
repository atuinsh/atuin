# shells: bash zsh
# scan: before

arr=(
  a
  # it's a comment
  b
)
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
