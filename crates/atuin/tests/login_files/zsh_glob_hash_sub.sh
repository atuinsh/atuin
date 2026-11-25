# shells: zsh
# scan: unknown

setopt extendedglob
x=$(print -l /usr/(#i)BIN)
if [ "$XDG_VTNR" = 1 ]; then
  exec sway
fi
echo END
