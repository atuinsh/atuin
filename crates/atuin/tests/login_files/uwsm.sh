# shells: bash zsh dash
# scan: before

if uwsm_check() { [ "$XDG_VTNR" = 1 ]; }; uwsm_check; then
	exec sway
fi
echo END
