ATUIN_SESSION=$(atuin uuid)
export ATUIN_SESSION

_atuin_preexec() {
    id=$(atuin history start "$1")
    export ATUIN_HISTORY_ID="$id"
}

_atuin_precmd() {
    local EXIT="$?"

    [[ -z "${ATUIN_HISTORY_ID}" ]] && return

    (RUST_LOG=error atuin history end "$ATUIN_HISTORY_ID" --exit $EXIT &) > /dev/null 2>&1
}


__atuin_history ()
{
    tput rmkx
    HISTORY="$(RUST_LOG=error atuin search -i "$BUFFER" 3>&1 1>&2 2>&3)"
    tput smkx

    READLINE_LINE=${HISTORY}
    READLINE_POINT=${#READLINE_LINE}
}


preexec_functions+=(_atuin_preexec)
precmd_functions+=(_atuin_precmd)

if [[ -z $ATUIN_NOBIND ]]; then
    bind -x '"\C-r": __atuin_history'
fi
