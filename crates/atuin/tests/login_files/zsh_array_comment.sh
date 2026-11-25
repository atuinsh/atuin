# shells: zsh
# scan: before

typeset -U path
path=(
  $path
  # Homebrew's binaries
  /usr/bin
)
if [ "$XDG_VTNR" = 1 ]; then
  exec sway
fi
echo END
