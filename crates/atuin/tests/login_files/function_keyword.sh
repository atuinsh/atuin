# shells: bash zsh
# scan: before

function start_wm {
  exec sway
}
[ "$XDG_VTNR" = 1 ] && start_wm
echo END
