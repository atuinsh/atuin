# shells: bash zsh dash
# scan: before

cat > /dev/null <<-'MSG'
	case x in
	esac )
	MSG
[ "$XDG_VTNR" = 1 ] && exec sway
echo END
