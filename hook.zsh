# Source this in your ~/.zshrc

_atuin_preexec(){
	id=$(atuin history start $1)
	export ATUIN_HISTORY_ID="$id"
}

_atuin_precmd(){
	local EXIT="$?"

	[[ -z "${ATUIN_HISTORY_ID}" ]] && return

	atuin history end $ATUIN_HISTORY_ID --exit $EXIT
}

add-zsh-hook preexec _atuin_preexec
add-zsh-hook precmd _atuin_precmd
