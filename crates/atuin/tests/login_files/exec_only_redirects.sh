# shells: bash zsh dash
# scan: no-exec

exec 2>&1
exec 3>/dev/null
echo END
