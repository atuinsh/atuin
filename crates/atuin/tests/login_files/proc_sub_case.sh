# shells: bash zsh
# scan: before

while read -r l; do :; done < <(case a in a) echo A=1;; esac)
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
