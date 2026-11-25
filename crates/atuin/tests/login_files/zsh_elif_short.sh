# shells: zsh
# scan: before

if [[ $XDG_VTNR = 2 ]] { echo two } elif [[ $XDG_VTNR = 1 ]] {
  exec sway }
echo END
