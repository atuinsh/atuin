ATUIN_SESSION=$(atuin uuid)
export ATUIN_SESSION

_atuin_preexec() {
    local id
    id=$(atuin history start -- "$1")
    export ATUIN_HISTORY_ID="${id}"
}

_atuin_precmd() {
    local EXIT="$?"

    [[ -z "${ATUIN_HISTORY_ID}" ]] && return

    (ATUIN_LOG=error atuin history end --exit "${EXIT}" -- "${ATUIN_HISTORY_ID}" &) >/dev/null 2>&1
    export ATUIN_HISTORY_ID=""
}

__atuin_history() {
    # shellcheck disable=SC2048,SC2086
    HISTORY="$(ATUIN_SHELL_BASH=t ATUIN_LOG=error atuin search $* -i -- "${READLINE_LINE}" 3>&1 1>&2 2>&3)"

    if [[ $HISTORY == __atuin_accept__:* ]]
    then
      HISTORY=${HISTORY#__atuin_accept__:}
      echo "$HISTORY"
      # Need to run the pre/post exec functions manually
      _atuin_preexec "$HISTORY"
      eval "$HISTORY"
      _atuin_precmd
      echo
    else
      READLINE_LINE=${HISTORY}
      READLINE_POINT=${#READLINE_LINE}
    fi

}

if [[ -n "${BLE_VERSION-}" ]]; then
    blehook PRECMD-+=_atuin_precmd
    blehook PREEXEC-+=_atuin_preexec
else
    precmd_functions+=(_atuin_precmd)
    preexec_functions+=(_atuin_preexec)
fi
