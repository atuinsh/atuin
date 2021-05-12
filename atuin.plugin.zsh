local FOUND_ATUIN=$+commands[atuin]

if [[ $FOUND_ATUIN -eq 1 ]]; then
  source <(atuin init zsh)
fi
