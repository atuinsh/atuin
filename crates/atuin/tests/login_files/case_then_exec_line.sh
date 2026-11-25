# shells: bash zsh dash
# scan: before

case "$TERM" in linux) export A=1;; esac; [ "$XDG_VTNR" = 1 ] && exec sway
echo END
