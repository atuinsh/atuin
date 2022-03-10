# shellcheck disable=2148,SC2168,SC1090,SC2125

# Set $0
# reference: https://zdharma-continuum.github.io/zinit/wiki/zsh-plugin-standard/
0="${${ZERO:-${0:#$ZSH_ARGZERO}}:-${(%):-%N}}"
0="${${(M)0:#/*}:-$PWD/$0}"

zmodload -Fa zsh/parameter p:commands

(( $+commands[atuin] )) && eval "$(atuin init zsh)"
