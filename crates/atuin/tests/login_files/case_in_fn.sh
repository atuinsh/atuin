# shells: bash zsh dash
# scan: before

f() { case $1 in 1) exec sway;; esac; }
f "$XDG_VTNR"
echo END
