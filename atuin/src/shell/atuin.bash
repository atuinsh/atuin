ATUIN_SESSION=$(atuin uuid)
ATUIN_STTY=$(stty -g)
export ATUIN_SESSION

__atuin_preexec() {
    if [[ ! ${BLE_ATTACHED-} ]]; then
        # With bash-preexec, preexec may be called even for the command run by
        # keybindings.  There is no general and robust way to detect the
        # command for keybindings, but at least we want to exclude Atuin's
        # keybindings.
        [[ $BASH_COMMAND == '__atuin_history'* && $BASH_COMMAND != "$1" ]] && return 0
    fi

    local id
    id=$(atuin history start -- "$1")
    export ATUIN_HISTORY_ID="${id}"
}

__atuin_precmd() {
    local EXIT="$?"

    [[ -z "${ATUIN_HISTORY_ID}" ]] && return

    local duration=""
    # shellcheck disable=SC2154,SC2309
    if [[ -n "${BLE_ATTACHED-}" && _ble_bash -ge 50000 && -n "${_ble_exec_time_ata-}" ]]; then
      # We use the high-resolution duration based on EPOCHREALTIME (bash >=
      # 5.0) that is recorded by ble.sh. The shell variable
      # `_ble_exec_time_ata` contains the execution time in microseconds.
      duration=${_ble_exec_time_ata}000
    fi

    (ATUIN_LOG=error atuin history end --exit "${EXIT}" ${duration:+--duration "$duration"} -- "${ATUIN_HISTORY_ID}" &) >/dev/null 2>&1
    export ATUIN_HISTORY_ID=""
}

__atuin_set_ret_value() {
    return ${1:+"$1"}
}

# The expansion ${PS1@P} is available in bash >= 4.4.
if ((BASH_VERSINFO[0] >= 5 || BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4)); then
    __atuin_use_prompt_expansion=true
else
    __atuin_use_prompt_expansion=false
fi

__atuin_accept_line() {
    local __atuin_command=$1

    # Reprint the prompt, accounting for multiple lines
    if [[ $__atuin_use_prompt_expansion == true ]]; then
        local __atuin_prompt=${PS1@P}
        local __atuin_prompt_offset
        __atuin_prompt_offset=$(printf '%s' "$__atuin_prompt" | wc -l)
        if ((__atuin_prompt_offset > 0)); then
            tput cuu "$__atuin_prompt_offset"
        fi
        printf '%s\n' "$__atuin_prompt$__atuin_command"
    else
        printf '%s\n' "\$ $__atuin_command"
    fi

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
    if [[ $__atuin_use_prompt_expansion == true ]]; then
        __atuin_set_ret_value "${__bp_last_ret_value-}" "${__bp_last_argument_prev_command-}"
        printf '%s\r' "${PS1@P}"
    fi
}

__atuin_history() {
    # Default action of the up key: When this function is called with the first
    # argument `--shell-up-key-binding`, we perform Atuin's history search only
    # when the up key is supposed to cause the history movement in the original
    # binding.  We do this only for ble.sh because the up key always invokes
    # the history movement in the plain Bash.
    if [[ ${BLE_ATTACHED-} && ${1-} == --shell-up-key-binding ]]; then
        # When the current cursor position is not in the first line, the up key
        # should move the cursor to the previous line.  While the selection is
        # performed, the up key should not start the history search.
        # shellcheck disable=SC2154 # Note: these variables are set by ble.sh
        if [[ ${_ble_edit_str::_ble_edit_ind} == *$'\n'* || $_ble_edit_mark_active ]]; then
            ble/widget/@nomarked backward-line
            local status=$?
            READLINE_LINE=$_ble_edit_str
            READLINE_POINT=$_ble_edit_ind
            READLINE_MARK=$_ble_edit_mark
            return "$status"
        fi
    fi

    HISTORY="$(ATUIN_SHELL_BASH=t ATUIN_LOG=error atuin search "$@" -i -- "${READLINE_LINE}" 3>&1 1>&2 2>&3)"

    # We do nothing when the search is canceled.
    [[ $HISTORY ]] || return 0

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

    # Define and register an autosuggestion source for ble.sh's auto-complete.
    # If you'd like to overwrite this, define the same name of shell function
    # after the $(atuin init bash) line in your .bashrc.  If you do not need
    # the auto-complete source by atuin, please add the following code to
    # remove the entry after the $(atuin init bash) line in your .bashrc:
    #
    #   ble/util/import/eval-after-load core-complete '
    #     ble/array#remove _ble_complete_auto_source atuin-history'
    #
    function ble/complete/auto-complete/source:atuin-history {
        local suggestion
        suggestion=$(atuin search --cmd-only --limit 1 --search-mode prefix -- "$_ble_edit_str")
        [[ $suggestion == "$_ble_edit_str"?* ]] || return 1
        ble/complete/auto-complete/enter h 0 "${suggestion:${#_ble_edit_str}}" '' "$suggestion"
    }
    ble/util/import/eval-after-load core-complete '
        ble/array#unshift _ble_complete_auto_source atuin-history'
fi
precmd_functions+=(__atuin_precmd)
preexec_functions+=(__atuin_preexec)
