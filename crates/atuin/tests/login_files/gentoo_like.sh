# shells: bash zsh dash
# scan: before

if [ -z "$XDG_RUNTIME_DIR" ]; then
	XDG_RUNTIME_DIR="$HOME/$(id -u)-runtime-dir"
	mkdir -pm 0700 "$XDG_RUNTIME_DIR"
	export XDG_RUNTIME_DIR
fi
case "$(tty)" in
	/dev/tty1|*) [ "$XDG_VTNR" = 1 ] && exec env WLR_RENDERER=pixman sway ;;
esac
echo END
