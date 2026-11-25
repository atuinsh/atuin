# shells: zsh
# scan: before

x=${(f)"$(echo a)"}
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
