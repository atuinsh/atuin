# shellcheck disable=SC2034 # zsh reads PROMPT
PROMPT='%F{cyan}laptop%f %F{8}~/src/atuin%f %F{green}$%f '
# Its ssh reads ~/.ssh from the passwd entry, not this HOME.
alias ssh='ssh -F ~/.ssh/config'
cd ~/src/atuin || return
eval "$(atuin init zsh)"
