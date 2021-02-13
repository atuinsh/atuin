# Source this in your ~/.zshrc
export ATUIN_SESSION=$(atuin uuid)
export ATUIN_FUZZY=fzf

_atuin_preexec(){
	id=$(atuin history start $1)
	export ATUIN_HISTORY_ID="$id"
}

_atuin_precmd(){
	local EXIT="$?"

	[[ -z "${ATUIN_HISTORY_ID}" ]] && return

	atuin history end $ATUIN_HISTORY_ID --exit $EXIT
}

_atuin_search(){
	emulate -L zsh
	zle -I

	output=$(atuin history list --distinct | $ATUIN_FUZZY)

	if [[ -n $output ]] ; then
		LBUFFER=$output
	fi

	zle reset-prompt
}

add-zsh-hook preexec _atuin_preexec
add-zsh-hook precmd _atuin_precmd

zle -N _atuin_search_widget _atuin_search

bindkey '^r' _atuin_search_widget
