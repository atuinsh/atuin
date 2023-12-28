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

      if [[ -n "${BLE_ATTACHED-}" ]]; then
        ble-edit/content/reset-and-check-dirty "$HISTORY"
        ble/widget/accept-line
      else
        # Reprint the prompt, accounting for multiple lines
        local __atuin_prompt_offset
        __atuin_prompt_offset=$(echo -n "${PS1@P}" | tr -cd '\n' | wc -c)
        if ((__atuin_prompt_offset > 0)); then
          tput cuu "$__atuin_prompt_offset"
        fi
        echo "${PS1@P}$HISTORY"

        # Assuming bash-preexec
        # Invoke every function in the preexec array
        local preexec_function
        local preexec_function_ret_value
        local preexec_ret_value=0
        for preexec_function in "${preexec_functions[@]:-}"; do
          if type -t "$preexec_function" 1>/dev/null; then
            __atuin_set_ret_value "${__bp_last_ret_value:-}"
            "$preexec_function" "$HISTORY"
            preexec_function_ret_value="$?"
            if [[ "$preexec_function_ret_value" != 0 ]]; then
              preexec_ret_value="$preexec_function_ret_value"
            fi
          fi
        done
        # shellcheck disable=SC2154
        __atuin_set_ret_value "$preexec_ret_value" "$__bp_last_argument_prev_command"

        # Juggle the terminal settings so that the command can be interacted with
        local stty_backup
        stty_backup=$(stty -g)
        stty "$ATUIN_STTY"

        eval "$HISTORY"
        exit_status=$?

        stty "$stty_backup"

        # Execute preprompt commands
        __atuin_set_ret_value "$exit_status" "$HISTORY"
        eval "$PROMPT_COMMAND"
        # Add it to the bash history
        history -s "$HISTORY"
        # Bash will redraw only the line with the prompt after we finish,
        # so to work for a multiline prompt we need to print it ourselves,
        # then move up a line
        __atuin_set_ret_value "$exit_status" "$HISTORY"
        echo "${PS1@P}"
        tput cuu 1
        __atuin_set_ret_value "$exit_status" "$HISTORY"
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
