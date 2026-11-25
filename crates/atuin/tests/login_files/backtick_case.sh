# shells: bash zsh dash
# scan: before

x=`case a in a) echo b;; esac`
if [ "$XDG_VTNR" = 1 ]; then
  exec sway
fi
echo END
