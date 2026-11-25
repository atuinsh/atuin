# shells: bash zsh dash
# scan: before

if [ "$(case $XDG_VTNR in 1) echo y;; esac)" = y ]; then
  exec sway
fi
echo END
