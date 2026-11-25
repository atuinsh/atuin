# shells: bash
# scan: before

x=$(case a in a) echo 1;;& b) echo 2;; esac)
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
