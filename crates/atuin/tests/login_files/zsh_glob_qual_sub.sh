# shells: zsh
# scan: before

x=$(print -l /usr/bin/sh*(N))
if [ "$XDG_VTNR" = 1 ]; then
  exec sway
fi
echo END
