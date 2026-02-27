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

if [[ -z "${ATUIN_SESSION:-}" || "${ATUIN_SHLVL:-}" != "$SHLVL" ]]; then
    ATUIN_SESSION=$(atuin uuid)
    export ATUIN_SESSION
    export ATUIN_SHLVL=$SHLVL
fi
ATUIN_STTY=$(stty -g)
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
        if [[ $BASH_COMMAND != "$1" ]]; then
            case $BASH_COMMAND in
                '__atuin_history'* | '__atuin_widget_run'* | '__atuin_bash42_dispatch'*)
                    ATUIN_HISTORY_ID=__bash_preexec_failure__
                    return 0 ;;
            esac
        fi
    fi

    # Note: We update ATUIN_PREEXEC_BACKEND on every preexec because blesh's
    # attaching state can dynamically change.
    __atuin_update_preexec_backend

    local id
    id=$(atuin history start -- "$1" 2>/dev/null)
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

#------------------------------------------------------------------------------
# section: __atuin_accept_line
#
# The function "__atuin_accept_line" is kept for backward compatibility of the
# direct use of __atuin_history in keybindings by users.

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
        # Note: When a child Bash session is started by enter_accept, if the
        # environment variable READLINE_POINT is present, bash-preexec in the
        # child session does not fire preexec at all because it considers we
        # are inside Atuin's keybinding of the current session.  To avoid
        # propagating the environment variable to the child session, we remove
        # the export attribute of READLINE_LINE and READLINE_POINT.
        export -n READLINE_LINE READLINE_POINT

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

#------------------------------------------------------------------------------

# Check if tmux popup is available (tmux >= 3.2)
__atuin_tmux_popup_check() {
    [[ -n "${TMUX-}" ]] || return 1
    [[ "${ATUIN_TMUX_POPUP:-true}" != "false" ]] || return 1

    # https://github.com/tmux/tmux/wiki/FAQ#how-often-is-tmux-released-what-is-the-version-number-scheme
    local tmux_version
    tmux_version=$(tmux -V 2>/dev/null | sed -n 's/^[^0-9]*\([0-9][0-9]*\.[0-9][0-9]*\).*/\1/p') # Could have used grep...
    [[ -z "$tmux_version" ]] && return 1

    local m1 m2
    m1=${tmux_version%%.*}
    m2=${tmux_version#*.}
    m2=${m2%%.*}
    [[ "$m1" =~ ^[0-9]+$ ]] || return 1
    [[ "$m2" =~ ^[0-9]+$ ]] || m2=0
    (( m1 > 3 || (m1 == 3 && m2 >= 2) ))
}

# Use global variable to fix scope issues with traps
__atuin_popup_tmpdir=""
__atuin_tmux_popup_cleanup() {
    [[ -n "$__atuin_popup_tmpdir" && -d "$__atuin_popup_tmpdir" ]] && command rm -rf "$__atuin_popup_tmpdir"
    __atuin_popup_tmpdir=""
}

__atuin_search_cmd() {
    local -a search_args=("$@")

    if __atuin_tmux_popup_check; then
        __atuin_popup_tmpdir=$(mktemp -d) || return 1
        local result_file="$__atuin_popup_tmpdir/result"

        trap '__atuin_tmux_popup_cleanup' EXIT HUP INT TERM

        local escaped_query escaped_args
        escaped_query=$(printf '%s' "$READLINE_LINE" | sed "s/'/'\\\\''/g")
        escaped_args=""
        for arg in "${search_args[@]}"; do
            escaped_args+=" '$(printf '%s' "$arg" | sed "s/'/'\\\\''/g")'"
        done

        # In the popup, atuin goes to terminal, stderr goes to file
        local cdir popup_width popup_height
        cdir=$(pwd)
        popup_width="${ATUIN_TMUX_POPUP_WIDTH:-80%}" # Keep default value anyways
        popup_height="${ATUIN_TMUX_POPUP_HEIGHT:-60%}"
        tmux display-popup -d "$cdir" -w "$popup_width" -h "$popup_height" -E -E -- \
            sh -c "PATH='$PATH' ATUIN_SESSION='$ATUIN_SESSION' ATUIN_SHELL=bash ATUIN_LOG=error ATUIN_QUERY='$escaped_query' atuin search $escaped_args -i 2>'$result_file'"

        if [[ -f "$result_file" ]]; then
            cat "$result_file"
        fi

        __atuin_tmux_popup_cleanup
        trap - EXIT HUP INT TERM
    else
        ATUIN_SHELL=bash ATUIN_LOG=error ATUIN_QUERY=$READLINE_LINE atuin search "${search_args[@]}" -i 3>&1 1>&2 2>&3
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

    # READLINE_LINE and READLINE_POINT are only supported by bash >= 4.0 or
    # ble.sh.  When it is not supported, we clear them to suppress strange
    # behaviors.
    [[ ${BLE_ATTACHED-} ]] || ((BASH_VERSINFO[0] >= 4)) ||
        READLINE_LINE="" READLINE_POINT=0

    local __atuin_output
    __atuin_output=$(__atuin_search_cmd "$@")

    # We do nothing when the search is canceled.
    [[ $__atuin_output ]] || return 0

    if [[ $__atuin_output == __atuin_accept__:* ]]; then
        __atuin_output=${__atuin_output#__atuin_accept__:}

        if [[ ${BLE_ATTACHED-} ]]; then
            ble-edit/content/reset-and-check-dirty "$__atuin_output"
            ble/widget/accept-line
            READLINE_LINE=""
        elif [[ ${__atuin_macro_chain_keymap-} ]]; then
            READLINE_LINE=$__atuin_output
            bind -m "$__atuin_macro_chain_keymap" '"'"$__atuin_macro_chain"'": '"$__atuin_macro_accept_line"
        else
            __atuin_accept_line "$__atuin_output"
            READLINE_LINE=""
        fi

        READLINE_POINT=${#READLINE_LINE}
    else
        READLINE_LINE=$__atuin_output
        READLINE_POINT=${#READLINE_LINE}
        if [[ ! ${BLE_ATTACHED-} ]] && ((BASH_VERSINFO[0] < 4)) && [[ ${__atuin_macro_chain_keymap-} ]]; then
            bind -m "$__atuin_macro_chain_keymap" '"'"$__atuin_macro_chain"'": '"$__atuin_macro_insert_line"
        fi
    fi
}

__atuin_initialize_blesh() {
    # shellcheck disable=SC2154
    [[ ${BLE_VERSION-} ]] && ((_ble_version >= 400)) || return 0

    ble-import contrib/integration/bash-preexec

    # Define and register an autosuggestion source for ble.sh's auto-complete.
    # If you'd like to overwrite this, define the same name of shell function
    # after the $(atuin init bash) line in your .bashrc.  If you do not need
    # the auto-complete source by Atuin, please add the following code to
    # remove the entry after the $(atuin init bash) line in your .bashrc:
    #
    #   ble/util/import/eval-after-load core-complete '
    #     ble/array#remove _ble_complete_auto_source atuin-history'
    #
    function ble/complete/auto-complete/source:atuin-history {
        local suggestion
        suggestion=$(ATUIN_QUERY="$_ble_edit_str" atuin search --cmd-only --limit 1 --search-mode prefix 2>/dev/null)
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

#------------------------------------------------------------------------------
# section: atuin-bind

__atuin_widget=()

__atuin_widget_save() {
    local data=$1
    for REPLY in "${!__atuin_widget[@]}"; do
        if [[ ${__atuin_widget[REPLY]} == "$data" ]]; then
            return 0
        fi
    done
    # shellcheck disable=SC2154
    REPLY=${#__atuin_widget[*]}
    __atuin_widget[REPLY]=$data
}

__atuin_widget_run() {
    local data=${__atuin_widget[$1]}
    local keymap=${data%%:*} widget=${data#*:}
    local __atuin_macro_chain_keymap=$keymap
    bind -m "$keymap" '"'"$__atuin_macro_chain"'": ""'
    builtin eval -- "$widget"
}

# To realize the enter_accept feature in a robust way, we need to call the
# readline bindable function `accept-line'.  However, there is no way to call
# `accept-line' from the shell script.  To call the bindable function
# `accept-line', we may utilize string macros of readline.  When we bind KEYSEQ
# to a WIDGET that wants to conditionally call `accept-line' at the end, we
# perform two-step dispatching:
#
# 1. [KEYSEQ -> IKEYSEQ1 IKEYSEQ2]---We first translate KEYSEQ to two
#   intermediate key sequences IKEYSEQ1 and IKEYSEQ2 using string macros.  For
#   example, when we bind `__atuin_history` to \C-r, this step can be set up by
#   `bind '"\C-r": "IKEYSEQ1IKEYSEQ2"'`.
#
# 2. [IKEYSEQ1 -> WIDGET]---Then, IKEYSEQ1 is bound to the WIDGET, and the
#   binding of IKEYSEQ2 is dynamically determined by WIDGET.  For example, when
#   we bind `__atuin_history` to \C-r, this step can be set up by `bind -x
#   '"IKEYSEQ1": WIDGET'`.
#
# 3. [IKEYSEQ2 -> accept-line] or [IKEYSEQ2 -> ""]---To request the execution
#   of `accept-line', WIDGET can change the binding of IKEYSEQ2 by running
#   `bind '"IKEYSEQ2": accept-line''.  Otherwise, WIDGET can change the binding
#   of IKEYSEQ2 to no-op by running `bind '"IKEYSEQ2": ""'`.
#
# For the choice of the intermediate key sequences, we want to choose key
# sequences that are unlikely to conflict with others.  In addition, we want to
# avoid a key sequence containing \e because keymap "vi-insert" stops
# processing key sequences containing \e in older versions of Bash.  We have
# used \e[0;<m>A (a variant of the [up] key with modifier <m>) in Atuin 3.10.0
# for intermediate key sequences, but this contains \e and caused a problem.
# Instead, we use \C-x\C-_A<n>\a, which starts with \C-x\C-_ (an unlikely
# two-byte combination) and A (represents the initial letter of Atuin),
# followed by the payload <n> and the terminator \a (BEL, \C-g).

__atuin_macro_chain='\C-x\C-_A0\a'
for __atuin_keymap in emacs vi-insert vi-command; do
    bind -m "$__atuin_keymap" "\"$__atuin_macro_chain\": \"\""
done
unset -v __atuin_keymap

if ((BASH_VERSINFO[0] >= 5 || BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 3)); then
    # In Bash >= 4.3

    __atuin_macro_accept_line=accept-line

    __atuin_bind_impl() {
        local keymap=$1 keyseq=$2 command=$3

        # Note: In Bash <= 5.0, the table for `bind -x` from the keyseq to the
        # command is shared by all the keymaps (emacs, vi-insert, and
        # vi-command), so one cannot safely bind different command strings to
        # the same keyseq in different keymaps.  Therefore, the command string
        # and the keyseq need to be globally in one-to-one correspondence in
        # all the keymaps.
        local REPLY
        __atuin_widget_save "$keymap:$command"
        local widget=$REPLY
        local ikeyseq1='\C-x\C-_A'$((1 + widget))'\a'
        local ikeyseq2=$__atuin_macro_chain

        if ((BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] == 1)); then
            # Workaround for Bash 5.1: Bash 5.1 has a bug that overwriting an
            # existing "bind -x" keybinding breaks other existing "bind -x"
            # keybindings [1,2].  To work around the problem, we explicitly
            # unbind an existing keybinding before overwriting it.
            #
            # [1] https://lists.gnu.org/archive/html/bug-bash/2021-04/msg00135.html
            # [2] https://github.com/atuinsh/atuin/issues/962#issuecomment-3451132291
            bind -m "$keymap" -r "$keyseq"
        fi

        bind -m "$keymap" "\"$keyseq\": \"$ikeyseq1$ikeyseq2\""
        bind -m "$keymap" -x "\"$ikeyseq1\": __atuin_widget_run $widget"
    }

    __atuin_bind_blesh_onload() {
        # In ble.sh, we need to enable unrecognized CSI sequences like \e[0;0A,
        # which are discarded by ble.sh by default.  Note: In Bash <= 4.2, we
        # do not need to unset "decode_error_cseq_discard" because \e[0;<m>A is
        # used only for the macro chaining (which is unused by ble.sh) in Bash
        # <= 4.2.
        bleopt decode_error_cseq_discard=
    }
    if [[ ${BLE_VERSION-} ]]; then
        __atuin_bind_blesh_onload
    fi
    BLE_ONLOAD+=(__atuin_bind_blesh_onload)
else
    # In Bash <= 4.2, "bind -x" cannot bind a shell command to a keyseq having
    # more than two bytes, so we need to work with only two-byte sequences.
    #
    # However, the number of available combinations of two-byte sequences is
    # limited.  To minimize the number of key sequences used by Atuin, instead
    # of specifying a widget by its own intermediate sequence, we specify a
    # widget by a fixed-length sequence of multiple two-byte sequences.  More
    # specifically, instead of IKEYSEQ1, we use IKS1 IKS2 IKS3 [IKS4 IKS5]
    # IKSX, where IKS1..IKS5 just stores its information to a global variable,
    # and IKSX collects all the information and determine and call the actual
    # widget based on the stored information. Each of IKn (n=1..5) is one of
    # the two reserved sequences, $__atuin_bash42_code0 and
    # $__atuin_bash42_code1.  IKSX is fixed to be $__atuin_bash42_code2.
    #
    # For the choices of the special key sequences, we consider \C-xQ, \C-xR,
    # and \C-xS.  In the emacs editing mode of Bash, \C-x is used as a prefix
    # key, i.e., it is used for the beginning key of the keybindings with
    # multiple keys, so \C-x is unlikely to be used for a single-key binding by
    # the user.  Also, \C-x is not used in the vi editing mode by default.  The
    # combinations \C-xQ..\C-xS are also unlikely be used because we need to
    # switch the modifier keys from Control to Shift to input these sequences,
    # and these are not easy to input.
    __atuin_bash42_code0='\C-xQ'
    __atuin_bash42_code1='\C-xR'
    __atuin_bash42_code2='\C-xS'

    __atuin_bash42_encode() {
        REPLY=
        local n=$1 min_width=${2-}
        while
            if ((n % 2 == 0)); then
                REPLY=$__atuin_bash42_code0$REPLY
            else
                REPLY=$__atuin_bash42_code1$REPLY
            fi
            (((n /= 2) || ${#REPLY} / ${#__atuin_bash42_code0} < min_width))
        do :; done
    }

    __atuin_bash42_bind() {
        local __atuin_keymap
        for __atuin_keymap in emacs vi-insert vi-command; do
            bind -m "$__atuin_keymap" -x '"'"$__atuin_bash42_code0"'": __atuin_bash42_dispatch_selector+=0'
            bind -m "$__atuin_keymap" -x '"'"$__atuin_bash42_code1"'": __atuin_bash42_dispatch_selector+=1'
            bind -m "$__atuin_keymap" -x '"'"$__atuin_bash42_code2"'": __atuin_bash42_dispatch'
        done
    }
    __atuin_bash42_bind
    # In Bash <= 4.2, there is no way to read users' "bind -x" settings, so we
    # need to explicitly perform "bind -x" when ble.sh is loaded.
    BLE_ONLOAD+=(__atuin_bash42_bind)

    if ((BASH_VERSINFO[0] >= 4)); then
        __atuin_macro_accept_line=accept-line
    else
        # Note: We rewrite the command line and invoke `accept-line'.  In
        # bash <= 3.2, there is no way to rewrite the command line from the
        # shell script, so we rewrite it using a macro and
        # `shell-expand-line'.
        #
        # Note: Concerning the key sequences to invoke bindable functions
        # such as "\C-x\C-_A1\a", another option is to use
        # "\exbegginning-of-line\r", etc. to make it consistent with bash
        # >= 5.3.  However, an older Bash configuration can still conflict
        # on [M-x].  The conflict is more likely than \C-x\C-_A1\a.
        for __atuin_keymap in emacs vi-insert vi-command; do
            bind -m "$__atuin_keymap" '"\C-x\C-_A1\a": beginning-of-line'
            bind -m "$__atuin_keymap" '"\C-x\C-_A2\a": kill-line'
            # shellcheck disable=SC2016
            bind -m "$__atuin_keymap" '"\C-x\C-_A3\a": "$READLINE_LINE"'
            bind -m "$__atuin_keymap" '"\C-x\C-_A4\a": shell-expand-line'
            bind -m "$__atuin_keymap" '"\C-x\C-_A5\a": accept-line'
            bind -m "$__atuin_keymap" '"\C-x\C-_A6\a": end-of-line'
        done
        unset -v __atuin_keymap

        bind -m vi-command '"\C-x\C-_A7\a": vi-insertion-mode'
        bind -m vi-insert  '"\C-x\C-_A7\a": vi-movement-mode'

        # "\C-x\C-_A10\a": Replace the command line with READLINE_LINE.  When we are
        #   in the vi-command keymap, we go to vi-insert, input
        #   "$READLINE_LINE", and come back to vi-command.
        bind -m emacs      '"\C-x\C-_A10\a": "\C-x\C-_A1\a\C-x\C-_A2\a\C-x\C-_A3\a\C-x\C-_A4\a"'
        bind -m vi-insert  '"\C-x\C-_A10\a": "\C-x\C-_A1\a\C-x\C-_A2\a\C-x\C-_A3\a\C-x\C-_A4\a"'
        bind -m vi-command '"\C-x\C-_A10\a": "\C-x\C-_A1\a\C-x\C-_A2\a\C-x\C-_A7\a\C-x\C-_A3\a\C-x\C-_A7\a\C-x\C-_A4\a"'

        __atuin_macro_accept_line='"\C-x\C-_A10\a\C-x\C-_A5\a"'
        __atuin_macro_insert_line='"\C-x\C-_A10\a\C-x\C-_A6\a"'
    fi

    __atuin_bash42_dispatch_selector=

    __atuin_bash42_dispatch() {
        local s=$__atuin_bash42_dispatch_selector
        __atuin_bash42_dispatch_selector=
        __atuin_widget_run "$((2#0$s))"
    }

    __atuin_bind_impl() {
        local keymap=$1 keyseq=$2 command=$3

        __atuin_widget_save "$keymap:$command"
        __atuin_bash42_encode "$REPLY"
        local macro=$REPLY$__atuin_bash42_code2$__atuin_macro_chain

        bind -m "$keymap" "\"$keyseq\": \"$macro\""
    }
fi

atuin-bind() {
    local keymap=
    local OPTIND=1 OPTARG="" OPTERR=0 flag
    while getopts ':m:' flag "$@"; do
        case $flag in
            m) keymap=$OPTARG ;;
            *)
                printf '%s\n' "atuin-bind: unrecognized option '-$flag'" >&2
                return 2
                ;;
        esac
    done
    shift "$((OPTIND - 1))"

    if (($# != 2)); then
        printf '%s\n' 'usage: atuin-bind [-m keymap] keyseq widget' >&2
        return 2
    fi

    local keyseq=$1
    [[ $keymap ]] || keymap=$(bind -v | awk '$2 == "keymap" { print $3 }')
    case $keymap in
        emacs-meta) keymap=emacs keyseq='\e'$keyseq ;;
        emacs-ctlx) keymap=emacs keyseq='\C-x'$keyseq ;;
        emacs*)     keymap=emacs ;;
        vi-insert)  ;;
        vi*)        keymap=vi-command ;;
        *)
            printf '%s\n' "atuin-bind: unknown keymap '$keymap'" >&2
            return 2 ;;
    esac

    local command=$2 widget=${2%%[[:blank:]]*}
    case $widget in
        atuin-search)          command=${2/#"$widget"/__atuin_history} ;;
        atuin-search-emacs)    command=${2/#"$widget"/__atuin_history --keymap-mode=emacs} ;;
        atuin-search-viins)    command=${2/#"$widget"/__atuin_history --keymap-mode=vim-insert} ;;
        atuin-search-vicmd)    command=${2/#"$widget"/__atuin_history --keymap-mode=vim-normal} ;;
        atuin-up-search)       command=${2/#"$widget"/__atuin_history --shell-up-key-binding} ;;
        atuin-up-search-emacs) command=${2/#"$widget"/__atuin_history --shell-up-key-binding --keymap-mode=emacs} ;;
        atuin-up-search-viins) command=${2/#"$widget"/__atuin_history --shell-up-key-binding --keymap-mode=vim-insert} ;;
        atuin-up-search-vicmd) command=${2/#"$widget"/__atuin_history --shell-up-key-binding --keymap-mode=vim-normal} ;;
    esac

    __atuin_bind_impl "$keymap" "$keyseq" "$command"
}

#------------------------------------------------------------------------------

# shellcheck disable=SC2154
if [[ $__atuin_bind_ctrl_r == true ]]; then
    # Note: We do not overwrite [C-r] in the vi-command keymap because we do
    # not want to overwrite "redo", which is already bound to [C-r] in the
    # vi_nmap keymap in ble.sh.
    atuin-bind -m emacs      '\C-r' atuin-search-emacs
    atuin-bind -m vi-insert  '\C-r' atuin-search-viins
    atuin-bind -m vi-command '/'    atuin-search-emacs
fi

# shellcheck disable=SC2154
if [[ $__atuin_bind_up_arrow == true ]]; then
    atuin-bind -m emacs      '\e[A' atuin-up-search-emacs
    atuin-bind -m emacs      '\eOA' atuin-up-search-emacs
    atuin-bind -m vi-insert  '\e[A' atuin-up-search-viins
    atuin-bind -m vi-insert  '\eOA' atuin-up-search-viins
    atuin-bind -m vi-command '\e[A' atuin-up-search-vicmd
    atuin-bind -m vi-command '\eOA' atuin-up-search-vicmd
    atuin-bind -m vi-command 'k'    atuin-up-search-vicmd
fi

#------------------------------------------------------------------------------
fi # (include guard) end of main content
