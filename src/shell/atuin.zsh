# Source this in your ~/.zshrc
export ATUIN_SESSION=$(atuin uuid)
export ATUIN_HISTORY="atuin history list"
export ATUIN_BINDKEYS="true"

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

	output=$(eval $ATUIN_HISTORY | fzf)

	if [[ -n $output ]] ; then
		BUFFER=$output
	fi

	zle reset-prompt
}

_atuin_up_search(){
	emulate -L zsh
	zle -I

	output=$(eval $ATUIN_HISTORY | fzf --no-sort --tac)

	if [[ -n $output ]] ; then
		BUFFER=$output
	fi

	zle reset-prompt
}

add-zsh-hook preexec _atuin_preexec
add-zsh-hook precmd _atuin_precmd

zle -N _atuin_search_widget _atuin_search
zle -N _atuin_up_search_widget _atuin_up_search

if [[ $ATUIN_BINDKEYS == "true" ]]; then
	bindkey '^r' _atuin_search_widget
	bindkey '^[[A' _atuin_up_search_widget
fi
