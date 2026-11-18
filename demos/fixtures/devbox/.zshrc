# shellcheck disable=SC2034 # zsh reads PROMPT
PROMPT='%F{magenta}devbox%f %F{8}~/src/atuin%f %F{green}$%f '
cd ~/src/atuin || return
eval "$(atuin init zsh)"
