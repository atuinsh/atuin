# shells: bash zsh dash
# scan: before

session() {
  case "$1" in
    1) echo sway ;;
    *) echo none ;;
  esac
}
wm=$(session "$XDG_VTNR")
if [ "$wm" != none ]; then
  exec "$wm"
fi
echo END
