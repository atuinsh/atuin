# Include guard
if [[ ${__atuin_initialized-} == true ]]; then
    false
elif [[ $- != *i* ]]; then
    # Enable only in interactive shells
    false
elif ((BASH_VERSINFO[0] < 3 || BASH_VERSINFO[0] == 3 && BASH_VERSINFO[1] < 1)); then
    # Require bash >= 3.1
    [[ -t 2 ]] && printf 'atuin: requires bash >= 3.1 for the integration.\n' >&2
    false
else # (include guard) beginning of main content
#------------------------------------------------------------------------------
__atuin_initialized=true

ATUIN_SESSION=$(atuin uuid)
ATUIN_STTY=$(stty -g)
export ATUIN_SESSION
ATUIN_HISTORY_ID=""

export ATUIN_PREEXEC_BACKEND=$SHLVL:none
__atuin_update_preexec_backend() {
    if [[ ${BLE_ATTACHED-} ]]; then
        ATUIN_PREEXEC_BACKEND=$SHLVL:blesh-${BLE_VERSION-}
    elif [[ ${bash_preexec_imported-} ]]; then
        ATUIN_PREEXEC_BACKEND=$SHLVL:bash-preexec
    elif [[ ${__bp_imported-} ]]; then
        ATUIN_PREEXEC_BACKEND="$SHLVL:bash-preexec (old)"
    else
        ATUIN_PREEXEC_BACKEND=$SHLVL:unknown
    fi
}

__atuin_preexec() {
    # Workaround for old versions of bash-preexec
    if [[ ! ${BLE_ATTACHED-} ]]; then
        # In older versions of bash-preexec, the preexec hook may be called
        # even for the commands run by keybindings.  There is no general and
        # robust way to detect the command for keybindings, but at least we
        # want to exclude Atuin's keybindings.  When the preexec hook is called
        # for a keybinding, the preexec hook for the user command will not
        # fire, so we instead set a fake ATUIN_HISTORY_ID here to notify
        # __atuin_precmd of this failure.
        if [[ $BASH_COMMAND == '__atuin_history'* && $BASH_COMMAND != "$1" ]]; then
            ATUIN_HISTORY_ID=__bash_preexec_failure__
            return 0
        fi
    fi

    # Note: We update ATUIN_PREEXEC_BACKEND on every preexec because blesh's
    # attaching state can dynamically change.
    __atuin_update_preexec_backend

    local id
    id=$(atuin history start -- "$1")
    export ATUIN_HISTORY_ID=$id
    __atuin_preexec_time=${EPOCHREALTIME-}
}

__atuin_precmd() {
    local EXIT=$? __atuin_precmd_time=${EPOCHREALTIME-}

    [[ ! $ATUIN_HISTORY_ID ]] && return

    # If the previous preexec hook failed, we manually call __atuin_preexec
    if [[ $ATUIN_HISTORY_ID == __bash_preexec_failure__ ]]; then
        # This is the command extraction code taken from bash-preexec
        local previous_command
        previous_command=$(
            export LC_ALL=C HISTTIMEFORMAT=''
            builtin history 1 | sed '1 s/^ *[0-9][0-9]*[* ] //'
        )
        __atuin_preexec "$previous_command"
    fi

    local duration=""
    # shellcheck disable=SC2154,SC2309
    if [[ ${BLE_ATTACHED-} && ${_ble_exec_time_ata-} ]]; then
        # With ble.sh, we utilize the shell variable `_ble_exec_time_ata`
        # recorded by ble.sh.  It is more accurate than the measurements by
        # Atuin, which includes the spawn cost of Atuin.  ble.sh uses the
        # special shell variable `EPOCHREALTIME` in bash >= 5.0 with the
        # microsecond resolution, or the builtin `time` in bash < 5.0 with the
        # millisecond resolution.
        duration=${_ble_exec_time_ata}000
    elif ((BASH_VERSINFO[0] >= 5)); then
        # We calculate the high-resolution duration based on EPOCHREALTIME
        # (bash >= 5.0) recorded by precmd/preexec, though it might not be as
        # accurate as `_ble_exec_time_ata` provided by ble.sh because it
        # includes the extra time of the precmd/preexec handling.  Since Bash
        # does not offer floating-point arithmetic, we remove the non-digit
        # characters and perform the integral arithmetic.  The fraction part of
        # EPOCHREALTIME is fixed to have 6 digits in Bash.  We remove all the
        # non-digit characters because the decimal point is not necessarily a
        # period depending on the locale.
        duration=$((${__atuin_precmd_time//[!0-9]} - ${__atuin_preexec_time//[!0-9]}))
        if ((duration >= 0)); then
            duration=${duration}000
        else
            duration="" # clear the result on overflow
        fi
    fi

    (ATUIN_LOG=error atuin history end --exit "$EXIT" ${duration:+"--duration=$duration"} -- "$ATUIN_HISTORY_ID" &) >/dev/null 2>&1
    export ATUIN_HISTORY_ID=""
}

__atuin_set_ret_value() {
    return ${1:+"$1"}
}

# The shell function `__atuin_evaluate_prompt` evaluates prompt sequences in
# $PS1.  We switch the implementation of the shell function
# `__atuin_evaluate_prompt` based on the Bash version because the expansion
# ${PS1@P} is only available in bash >= 4.4.
if ((BASH_VERSINFO[0] >= 5 || BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4)); then
    __atuin_evaluate_prompt() {
        __atuin_set_ret_value "${__bp_last_ret_value-}" "${__bp_last_argument_prev_command-}"
        __atuin_prompt=${PS1@P}
    
        # Note: Strip the control characters ^A (\001) and ^B (\002), which
        # Bash internally uses to enclose the escape sequences.  They are
        # produced by '\[' and '\]', respectively, in $PS1 and used to tell
        # Bash that the strings inbetween do not contribute to the prompt
        # width.  After the prompt width calculation, Bash strips those control
        # characters before outputting it to the terminal.  We here strip these
        # characters following Bash's behavior.
        __atuin_prompt=${__atuin_prompt//[$'\001\002']}

        # Count the number of newlines contained in $__atuin_prompt
        __atuin_prompt_offset=${__atuin_prompt//[!$'\n']}
        __atuin_prompt_offset=${#__atuin_prompt_offset}
    }
else
    __atuin_evaluate_prompt() {
        __atuin_prompt='$ '
        __atuin_prompt_offset=0
    }
fi

# The shell function `__atuin_clear_prompt N` outputs terminal control
# sequences to clear the contents of the current and N previous lines.  After
# clearing, the cursor is placed at the beginning of the N-th previous line.
__atuin_clear_prompt_cache=()
__atuin_clear_prompt() {
    local offset=$1
    if [[ ! ${__atuin_clear_prompt_cache[offset]+set} ]]; then
        if [[ ! ${__atuin_clear_prompt_cache[0]+set} ]]; then
            __atuin_clear_prompt_cache[0]=$'\r'$(tput el 2>/dev/null || tput ce 2>/dev/null)
        fi
        if ((offset > 0)); then
            __atuin_clear_prompt_cache[offset]=${__atuin_clear_prompt_cache[0]}$(
                tput cuu "$offset" 2>/dev/null || tput UP "$offset" 2>/dev/null
                tput dl "$offset"  2>/dev/null || tput DL "$offset" 2>/dev/null
                tput il "$offset"  2>/dev/null || tput AL "$offset" 2>/dev/null
            )
        fi
    fi
    printf '%s' "${__atuin_clear_prompt_cache[offset]}"
}

__atuin_accept_line() {
    local __atuin_command=$1

    # Reprint the prompt, accounting for multiple lines
    local __atuin_prompt __atuin_prompt_offset
    __atuin_evaluate_prompt
    __atuin_clear_prompt "$__atuin_prompt_offset"
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
            __atuin_preexec_function_ret_value=$?
            if [[ $__atuin_preexec_function_ret_value != 0 ]]; then
                __atuin_preexec_ret_value=$__atuin_preexec_function_ret_value
            fi
        fi
    done

    # If extdebug is turned on and any preexec function returns non-zero
    # exit status, we do not run the user command.
    if ! { shopt -q extdebug && ((__atuin_preexec_ret_value)); }; then
        # Juggle the terminal settings so that the command can be interacted
        # with
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
    __atuin_evaluate_prompt
    printf '%s' "$__atuin_prompt"
    __atuin_clear_prompt 0
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

    # READLINE_LINE and READLINE_POINT are only supported by bash >= 4.0 or
    # ble.sh.  When it is not supported, we localize them to suppress strange
    # behaviors.
    [[ ${BLE_ATTACHED-} ]] || ((BASH_VERSINFO[0] >= 4)) ||
        local READLINE_LINE="" READLINE_POINT=0

    local __atuin_output
    __atuin_output=$(ATUIN_SHELL_BASH=t ATUIN_LOG=error ATUIN_QUERY="$READLINE_LINE" atuin search "$@" -i 3>&1 1>&2 2>&3)

    # We do nothing when the search is canceled.
    [[ $__atuin_output ]] || return 0

    if [[ $__atuin_output == __atuin_accept__:* ]]; then
        __atuin_output=${__atuin_output#__atuin_accept__:}

        if [[ ${BLE_ATTACHED-} ]]; then
            ble-edit/content/reset-and-check-dirty "$__atuin_output"
            ble/widget/accept-line
        else
            __atuin_accept_line "$__atuin_output"
        fi

        READLINE_LINE=""
        READLINE_POINT=${#READLINE_LINE}
    else
        READLINE_LINE=$__atuin_output
        READLINE_POINT=${#READLINE_LINE}
    fi
}

__atuin_initialize_blesh() {
    # shellcheck disable=SC2154
    [[ ${BLE_VERSION-} ]] && ((_ble_version >= 400)) || return 0

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
        suggestion=$(ATUIN_QUERY="$_ble_edit_str" atuin search --cmd-only --limit 1 --search-mode prefix)
        [[ $suggestion == "$_ble_edit_str"?* ]] || return 1
        ble/complete/auto-complete/enter h 0 "${suggestion:${#_ble_edit_str}}" '' "$suggestion"
    }
    ble/util/import/eval-after-load core-complete '
        ble/array#unshift _ble_complete_auto_source atuin-history'

    # @env BLE_SESSION_ID: `atuin doctor` references the environment variable
    # BLE_SESSION_ID.  We explicitly export the variable because it was not
    # exported in older versions of ble.sh.
    [[ ${BLE_SESSION_ID-} ]] && export BLE_SESSION_ID
}
__atuin_initialize_blesh
BLE_ONLOAD+=(__atuin_initialize_blesh)
precmd_functions+=(__atuin_precmd)
preexec_functions+=(__atuin_preexec)

# shellcheck disable=SC2154
if [[ $__atuin_bind_ctrl_r == true ]]; then
    # Note: We do not overwrite [C-r] in the vi-command keymap for Bash because
    # we do not want to overwrite "redo", which is already bound to [C-r] in
    # the vi_nmap keymap in ble.sh.
    bind -m emacs -x '"\C-r": __atuin_history --keymap-mode=emacs'
    bind -m vi-insert -x '"\C-r": __atuin_history --keymap-mode=vim-insert'
    bind -m vi-command -x '"/": __atuin_history --keymap-mode=emacs'
fi

# shellcheck disable=SC2154
if [[ $__atuin_bind_up_arrow == true ]]; then
    if ((BASH_VERSINFO[0] > 4 || BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 3)); then
        bind -m emacs -x '"\e[A": __atuin_history --shell-up-key-binding --keymap-mode=emacs'
        bind -m emacs -x '"\eOA": __atuin_history --shell-up-key-binding --keymap-mode=emacs'
        bind -m vi-insert -x '"\e[A": __atuin_history --shell-up-key-binding --keymap-mode=vim-insert'
        bind -m vi-insert -x '"\eOA": __atuin_history --shell-up-key-binding --keymap-mode=vim-insert'
        bind -m vi-command -x '"\e[A": __atuin_history --shell-up-key-binding --keymap-mode=vim-normal'
        bind -m vi-command -x '"\eOA": __atuin_history --shell-up-key-binding --keymap-mode=vim-normal'
        bind -m vi-command -x '"k": __atuin_history --shell-up-key-binding --keymap-mode=vim-normal'
    else
        # In bash < 4.3, "bind -x" cannot bind a shell command to a keyseq
        # having more than two bytes.  To work around this, we first translate
        # the keyseqs to the two-byte sequence \C-x\C-p (which is not used by
        # default) using string macros and run the shell command through the
        # keybinding to \C-x\C-p.
        bind -m emacs -x '"\C-x\C-p": __atuin_history --shell-up-key-binding --keymap-mode=emacs'
        bind -m emacs '"\e[A": "\C-x\C-p"'
        bind -m emacs '"\eOA": "\C-x\C-p"'
        bind -m vi-insert -x '"\C-x\C-p": __atuin_history --shell-up-key-binding --keymap-mode=vim-insert'
        bind -m vi-insert '"\e[A": "\C-x\C-p"'
        bind -m vi-insert '"\eOA": "\C-x\C-p"'
        bind -m vi-command -x '"\C-x\C-p": __atuin_history --shell-up-key-binding --keymap-mode=vim-normal'
        bind -m vi-command '"\e[A": "\C-x\C-p"'
        bind -m vi-command '"\eOA": "\C-x\C-p"'
        bind -m vi-command '"k": "\C-x\C-p"'
    fi
fi

#------------------------------------------------------------------------------
fi # (include guard) end of main content
