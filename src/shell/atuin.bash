ATUIN_SESSION=$(atuin uuid)
export ATUIN_SESSION

_atuin_preexec() {
    local id; id=$(atuin history start -- "$1")
    export ATUIN_HISTORY_ID="${id}"
}

_atuin_precmd() {
    local EXIT="$?"

    [[ -z "${ATUIN_HISTORY_ID}" ]] && return

    (RUST_LOG=error atuin history end --exit "${EXIT}" -- "${ATUIN_HISTORY_ID}" &) > /dev/null 2>&1
}

__atuin_history ()
{
    tput rmkx
    HISTORY="$(RUST_LOG=error atuin search -i -- "${READLINE_LINE}" 3>&1 1>&2 2>&3)"
    tput smkx

    READLINE_LINE=${HISTORY}
    READLINE_POINT=${#READLINE_LINE}
}

if [[ -n "${BLE_VERSION-}" ]]; then
    blehook PRECMD-+=_atuin_precmd
    blehook PREEXEC-+=_atuin_preexec
else
    precmd_functions+=(_atuin_precmd)
    preexec_functions+=(_atuin_preexec)
fi

if [[ -z ${ATUIN_NOBIND} ]]; then
    bind -x '"\C-r": __atuin_history'
    bind -x '"\e[A": __atuin_history'
    bind -x '"\eOA": __atuin_history'
fi
