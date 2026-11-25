# shells: bash zsh
# scan: before

x=$(if [[ $XDG_VTNR =~ (1|2) ]]; then echo a; fi)
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
