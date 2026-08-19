pub const ZSH_INIT: &str = r#"
_atuin_ai_cleanup() {
    true
}

# zle reset-prompt anchors the repaint at the cursor row: a multi-line
# prompt grows *upward*, overwriting whatever the inline TUI left on the
# rows above. Pad with prompt-height - 1 newlines first so the repaint
# lands on blank rows below the conversation instead.
_atuin_ai_reset_prompt() {
    local -a _prompt_lines=("${(@f)${(%%)PROMPT}}")
    local -i _pad=$(( ${#_prompt_lines} - 1 ))
    (( _pad > 0 )) && printf '\n%.0s' {1..$_pad} >/dev/tty
    zle reset-prompt
}

# Question mark at start of line - natural language mode.
# Named with 'self-' prefix so bracketed-paste-magic activates it during
# paste, allowing url-quote-magic to escape ? in pasted URLs via self-insert.
self-atuin-ai-question-mark() {
    # If buffer is empty or just contains '?', trigger natural language mode
    if [[ -z "$BUFFER" || "$BUFFER" == "?" ]]; then
        BUFFER=""
        # Close the semantic prompt zone (OSC 133 C, "command output
        # starts") before handing the terminal to the TUI. Without it,
        # terminals with shell integration (Ghostty) believe we are
        # still at the prompt, and their resize-time prompt reflow
        # erases everything below the prompt mark — including the
        # conversation the TUI printed.
        printf '\033]133;C\007' > /dev/tty
        local output
        output=$(atuin ai inline --hook 3>&1 1>&2 2>&3)

        # Clean up the inline viewport
        _atuin_ai_cleanup

        if [[ $output == __atuin_ai_print__:* ]]; then
            echo "${output#__atuin_ai_print__:}"
            _atuin_ai_reset_prompt
        elif [[ $output == __atuin_ai_cancel__ ]]; then
            _atuin_ai_reset_prompt
        elif [[ $output == __atuin_ai_execute__:* ]]; then
            RBUFFER=""
            LBUFFER=${output#__atuin_ai_execute__:}
            _atuin_ai_reset_prompt
            zle accept-line
        elif [[ $output == __atuin_ai_insert__:* ]]; then
            RBUFFER=""
            LBUFFER=${output#__atuin_ai_insert__:}
            _atuin_ai_reset_prompt
        elif [[ -n $output ]]; then
            RBUFFER=""
            LBUFFER=$output
            _atuin_ai_reset_prompt
        else
            _atuin_ai_reset_prompt
        fi
    else
        zle self-insert
    fi
}

# Set up keybindings
zle -N self-atuin-ai-question-mark
bindkey '?' self-atuin-ai-question-mark # Question mark
"#
.trim_ascii();

pub const BASH_INIT: &str = r#"
# Question mark at start of line - natural language mode
_atuin_ai_question_mark() {
    # If buffer is empty or just contains '?', trigger natural language mode
    if [[ -z "$READLINE_LINE" || "$READLINE_LINE" == "?" ]]; then
        READLINE_LINE=""
        READLINE_POINT=0

        # Close the semantic prompt zone (OSC 133 C) so terminals with
        # shell integration don't erase the TUI's output during their
        # resize-time prompt reflow.
        printf '\033]133;C\007' > /dev/tty
        local output
        output=$(atuin ai inline --hook 3>&1 1>&2 2>&3)

        if [[ $output == __atuin_ai_print__:* ]]; then
            echo "${output#__atuin_ai_print__:}"
            __atuin_insert_line ""
        elif [[ $output == __atuin_ai_cancel__ ]]; then
            __atuin_insert_line ""
        elif [[ $output == __atuin_ai_execute__:* ]]; then
            __atuin_accept_line "${output#__atuin_ai_execute__:}"
        elif [[ $output == __atuin_ai_insert__:* ]]; then
            # Insert the command for editing
            __atuin_insert_line "${output#__atuin_ai_insert__:}"
        elif [[ -n $output ]]; then
            # Default: insert for editing
            __atuin_insert_line "$output"
        fi
    else
        # Not at empty prompt, just insert the question mark
        READLINE_LINE="${READLINE_LINE:0:READLINE_POINT}?${READLINE_LINE:READLINE_POINT}"
        ((READLINE_POINT++))
    fi
}

# We only set up keybinding in Bash >= 4.0.  Bash < 4.0 does not provide
# READLINE_LINE, so if bound, it would always start an AI session regardless of
# the contents of the line buffer.  This means that it would make impossible
# to input "?" in Bash 3.2.
if ((BASH_VERSINFO[0] >= 4)) || [[ ${BLE_VERSION-} ]]; then
    atuin-bind '?' _atuin_ai_question_mark
fi
"#
.trim_ascii();

pub const FISH_INIT: &str = r#"
# Question mark at start of line - natural language mode
function _atuin_ai_question_mark
    set -l buf (commandline -b)

    # If buffer is empty or just contains '?', trigger natural language mode
    if test -z "$buf" -o "$buf" = "?"
        commandline -r ""

        # Close the semantic prompt zone (OSC 133 C) so terminals with
        # shell integration don't erase the TUI's output during their
        # resize-time prompt reflow.
        printf '\033]133;C\007' > /dev/tty

        # Run atuin ai inline, swapping stdout and stderr
        set -l output (atuin ai inline --hook 3>&1 1>&2 2>&3 | string collect)

        if string match --quiet '__atuin_ai_print__:*' "$output"
            echo (string replace "__atuin_ai_print__:" "" -- "$output" | string collect)
            commandline -f repaint
        else if test "$output" = "__atuin_ai_cancel__"
            commandline -f repaint
        else if string match --quiet '__atuin_ai_execute__:*' "$output"
            # Execute the command immediately
            set -l cmd (string replace "__atuin_ai_execute__:" "" -- "$output" | string collect)
            commandline -r "$cmd"
            commandline -f repaint
            commandline -f execute
        else if string match --quiet '__atuin_ai_insert__:*' "$output"
            # Insert the command for editing
            set -l cmd (string replace "__atuin_ai_insert__:" "" -- "$output" | string collect)
            commandline -r "$cmd"
            commandline -f repaint
        else if test -n "$output"
            # Default: insert for editing
            commandline -r "$output"
            commandline -f repaint
        else
            commandline -f repaint
        end
    else if not contains -- "$fish_key_bindings" fish_vi_key_bindings fish_hybrid_key_bindings
        # Not at empty prompt, just insert the question mark
        commandline -i "?"
    end
end

# Set up keybindings
bind "?" _atuin_ai_question_mark
"#
.trim_ascii();

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::zsh(
        ZSH_INIT,
        &["self-atuin-ai-question-mark", "bindkey", "zle self-insert"]
    )]
    #[case::bash(
        BASH_INIT,
        &["_atuin_ai_question_mark", "bind", "READLINE_LINE", "__atuin_accept_line", "atuin-bind"]
    )]
    #[case::fish(
        FISH_INIT,
        &["_atuin_ai_question_mark", "bind", "commandline"]
    )]
    fn shell_init(#[case] result: &str, #[case] extras: &[&str]) {
        for t in [
            "atuin ai inline --hook",
            "__atuin_ai_print__",
            "__atuin_ai_cancel__",
            "__atuin_ai_execute__",
            "__atuin_ai_insert__",
        ] {
            assert!(result.contains(t), "missing common token {t}");
        }
        for t in extras {
            assert!(result.contains(t), "missing shell token {t}");
        }
    }
}
