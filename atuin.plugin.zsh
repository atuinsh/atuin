# shellcheck disable=2148,SC2168,SC1090
local FOUND_ATUIN=$+commands[atuin]

if [[ $FOUND_ATUIN -eq 1 ]]; then
  source <(atuin init zsh)
fi
