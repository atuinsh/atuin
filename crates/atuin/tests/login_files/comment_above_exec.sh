# shells: bash zsh dash
# scan: before

# Start sway on the first console.
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
