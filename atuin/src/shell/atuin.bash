ATUIN_SESSION=$(atuin uuid)
ATUIN_STTY=$(stty -g)
export ATUIN_SESSION

__atuin_preexec() {
    local id
    id=$(atuin history start -- "$1")
    export ATUIN_HISTORY_ID="${id}"
}

__atuin_precmd() {
    local EXIT="$?"

    [[ -z "${ATUIN_HISTORY_ID}" ]] && return

    (ATUIN_LOG=error atuin history end --exit "${EXIT}" -- "${ATUIN_HISTORY_ID}" &) >/dev/null 2>&1
    export ATUIN_HISTORY_ID=""
}

__atuin_set_ret_value() {
    return ${1:+"$1"}
}

__atuin_history() {
    # shellcheck disable=SC2048,SC2086
    HISTORY="$(ATUIN_SHELL_BASH=t ATUIN_LOG=error atuin search $* -i -- "${READLINE_LINE}" 3>&1 1>&2 2>&3)"

    if [[ $HISTORY == __atuin_accept__:* ]]
    then
      HISTORY=${HISTORY#__atuin_accept__:}
      # Reprint the prompt, accounting for multiple lines
      # shellcheck disable=SC2046
      tput cuu $(echo -n "${PS1@P}" | tr -cd '\n' | wc -c)
      echo "${PS1@P}$HISTORY"

      if [[ -n "${BLE_VERSION-}" ]]; then
        blehook/invoke PREEXEC "$HISTORY"
      else
        # Assuming bash-preexec
        # Invoke every function in the preexec array
        local preexec_function
        local preexec_function_ret_value
        local preexec_ret_value=0
        for preexec_function in "${preexec_functions[@]:-}"; do
          if type -t "$preexec_function" 1>/dev/null; then
            __bp_set_ret_value "${__bp_last_ret_value:-}"
            "$preexec_function" "$HISTORY"
            preexec_function_ret_value="$?"
            if [[ "$preexec_function_ret_value" != 0 ]]; then
              preexec_ret_value="$preexec_function_ret_value"
            fi
          fi
        done
        # shellcheck disable=SC2154
        __bp_set_ret_value "$preexec_ret_value" "$__bp_last_argument_prev_command" 
      fi
      # Juggle the stty so that the command can be executed
      local stty_bkup=$(stty -g)
      stty "$ATUIN_STTY"
      eval "$HISTORY"
      exit_status=$?
      stty "$stty_bkup"
      # Execute preprompt commands
      __atuin_set_ret_value "$exit_status" "$HISTORY"
      eval "$PROMPT_COMMAND"
      # Need to reexecute the blehook
      if [[ -n "${BLE_VERSION-}" ]]; then
        __atuin_set_ret_value "$exit_status" "$HISTORY"
        blehook/invoke PRECMD "$?"
      fi
      # Add it to the bash history
      history -s "$HISTORY"
      # Bash will redraw only the line with the prompt after we finish,
      # so to work for a multiline prompt we need to print it ourselves,
      # then move up a line
      __atuin_set_ret_value "$exit_status" "$HISTORY"
      echo "${PS1@P}"
      tput cuu 1
      __atuin_set_ret_value "$exit_status" "$HISTORY"
      READLINE_LINE=""
      READLINE_POINT=${#READLINE_LINE}
    else
      READLINE_LINE=${HISTORY}
      READLINE_POINT=${#READLINE_LINE}
    fi
}

if [[ -n "${BLE_VERSION-}" ]]; then
    blehook PRECMD-+=__atuin_precmd
    blehook PREEXEC-+=__atuin_preexec
else
    precmd_functions+=(__atuin_precmd)
    preexec_functions+=(__atuin_preexec)
fi
