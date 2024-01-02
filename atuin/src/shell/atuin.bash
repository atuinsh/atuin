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

__atuin_accept_line() {
    local __atuin_command=$1

    # Reprint the prompt, accounting for multiple lines
    local __atuin_prompt=${PS1@P}
    local __atuin_prompt_offset
    __atuin_prompt_offset=$(printf '%s' "$__atuin_prompt" | wc -l)
    if ((__atuin_prompt_offset > 0)); then
      tput cuu "$__atuin_prompt_offset"
    fi
    printf '%s\n' "$__atuin_prompt$__atuin_command"

    # Add it to the bash history
    history -s "$__atuin_command"

    # Assuming bash-preexec
    # Invoke every function in the preexec array
    local __atuin_preexec_function
    local __atuin_preexec_function_ret_value
    local __atuin_preexec_ret_value=0
    for __atuin_preexec_function in "${preexec_functions[@]:-}"; do
      if type -t "$__atuin_preexec_function" 1>/dev/null; then
        __atuin_set_ret_value "${__bp_last_ret_value:-}"
        "$__atuin_preexec_function" "$__atuin_command"
        __atuin_preexec_function_ret_value="$?"
        if [[ "$__atuin_preexec_function_ret_value" != 0 ]]; then
          __atuin_preexec_ret_value="$__atuin_preexec_function_ret_value"
        fi
      fi
    done

    # If extdebug is turned on and any preexec function returns non-zero
    # exit status, we do not run the user command.
    if ! { shopt -q extdebug && ((__atuin_preexec_ret_value)); }; then
      # Juggle the terminal settings so that the command can be interacted with
      local __atuin_stty_backup
      __atuin_stty_backup=$(stty -g)
      stty "$ATUIN_STTY"

      # Execute the command.  Note: We need to record $? and $_ after the
      # user command within the same call of "eval" because $_ is otherwise
      # overwritten by the last argument of "eval".
      __atuin_set_ret_value "${__bp_last_ret_value-}" "${__bp_last_argument_prev_command-}"
      eval -- "$__atuin_command"$'\n__bp_last_ret_value=$? __bp_last_argument_prev_command=$_'

      stty "$__atuin_stty_backup"
    fi

    # Execute preprompt commands
    local __atuin_prompt_command
    for __atuin_prompt_command in "${PROMPT_COMMAND[@]}"; do
      __atuin_set_ret_value "${__bp_last_ret_value-}" "${__bp_last_argument_prev_command-}"
      eval -- "$__atuin_prompt_command"
    done
    # Bash will redraw only the line with the prompt after we finish,
    # so to work for a multiline prompt we need to print it ourselves,
    # then go to the beginning of the last line.
    __atuin_set_ret_value "${__bp_last_ret_value-}" "${__bp_last_argument_prev_command-}"
    printf '%s\r' "${PS1@P}"
}

__atuin_history() {
    # shellcheck disable=SC2048,SC2086
    HISTORY="$(ATUIN_SHELL_BASH=t ATUIN_LOG=error atuin search $* -i -- "${READLINE_LINE}" 3>&1 1>&2 2>&3)"

    if [[ $HISTORY == __atuin_accept__:* ]]
    then
      HISTORY=${HISTORY#__atuin_accept__:}

      if [[ -n "${BLE_ATTACHED-}" ]]; then
        ble-edit/content/reset-and-check-dirty "$HISTORY"
        ble/widget/accept-line
      else
        __atuin_accept_line "$HISTORY"
      fi

      READLINE_LINE=""
      READLINE_POINT=${#READLINE_LINE}
    else
      READLINE_LINE=${HISTORY}
      READLINE_POINT=${#READLINE_LINE}
    fi
}

# shellcheck disable=SC2154
if [[ -n "${BLE_VERSION-}" ]] && ((_ble_version >= 400)); then
    ble-import contrib/integration/bash-preexec
fi
precmd_functions+=(__atuin_precmd)
preexec_functions+=(__atuin_preexec)
