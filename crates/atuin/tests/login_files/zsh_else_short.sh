# shells: zsh
# scan: before

if [[ $XDG_VTNR != 1 ]] { echo other } else { exec sway }
echo END
