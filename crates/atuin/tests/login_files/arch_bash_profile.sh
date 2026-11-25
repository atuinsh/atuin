# shells: bash zsh
# scan: before

#
# ~/.bash_profile
#

[[ -f ~/.bashrc_x ]] && . ~/.bashrc_x
while read -r l; do
  export "$l"
done < <(printf 'A=1\nB=2\n')
if command -v nonexistent-tool >/dev/null 2>&1 || [[ ! $DISPLAY && $XDG_VTNR -eq 1 ]]; then
  exec sway
fi
echo END
