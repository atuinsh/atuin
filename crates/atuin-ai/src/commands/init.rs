use crate::commands::detect_shell;

pub(crate) async fn run(shell: String) -> eyre::Result<()> {
    let integration = match shell.as_str() {
        "zsh" => generate_zsh_integration(),
        "bash" => generate_bash_integration(),
        "fish" => generate_fish_integration(),
        "auto" => generate_auto_integration()?,
        _ => eyre::bail!("Unsupported shell: {}", shell),
    };

    println!("{}", integration);
    Ok(())
}

fn generate_auto_integration() -> eyre::Result<&'static str> {
    let shell = detect_shell();
    match shell.as_deref() {
        Some("zsh") => Ok(generate_zsh_integration()),
        Some("bash") => Ok(generate_bash_integration()),
        Some("fish") => Ok(generate_fish_integration()),
        Some(s) => eyre::bail!("Unsupported shell: {}", s),
        None => eyre::bail!("Could not detect shell"),
    }
}

/// Generate the zsh integration function - pure function for easy testing
pub fn generate_zsh_integration() -> &'static str {
    r#"
# TUI uses an alternate screen, so no explicit cleanup is needed.
_atuin_ai_cleanup() {
    true
}

# Question mark at start of line - natural language mode.
# Named with 'self-' prefix so bracketed-paste-magic activates it during
# paste, allowing url-quote-magic to escape ? in pasted URLs via self-insert.
self-atuin-ai-question-mark() {
    # If buffer is empty or just contains '?', trigger natural language mode
    if [[ -z "$BUFFER" || "$BUFFER" == "?" ]]; then
        BUFFER=""
        local output
        output=$(atuin ai inline --hook 3>&1 1>&2 2>&3)

        # Clean up the inline viewport
        _atuin_ai_cleanup

        if [[ $output == __atuin_ai_print__:* ]]; then
            zle -I
            echo "${output#__atuin_ai_print__:}"
        elif [[ $output == __atuin_ai_cancel__ ]]; then
            zle reset-prompt
        elif [[ $output == __atuin_ai_execute__:* ]]; then
            RBUFFER=""
            LBUFFER=${output#__atuin_ai_execute__:}
            zle reset-prompt
            zle accept-line
        elif [[ $output == __atuin_ai_insert__:* ]]; then
            RBUFFER=""
            LBUFFER=${output#__atuin_ai_insert__:}
            zle reset-prompt
        elif [[ -n $output ]]; then
            RBUFFER=""
            LBUFFER=$output
            zle reset-prompt
        else
            zle reset-prompt
        fi
    else
        zle self-insert
    fi
}

# Set up keybindings
zle -N self-atuin-ai-question-mark
bindkey '?' self-atuin-ai-question-mark # Question mark
"#
    .trim()
}

/// Generate the bash integration function - pure function for easy testing
pub fn generate_bash_integration() -> &'static str {
    r#"
# Question mark at start of line - natural language mode
_atuin_ai_question_mark() {
    # Default the trailing macro keyseq to a no-op; the execute branch below
    # rebinds it to accept-line.  See the binding setup at the bottom of this
    # script for how the macro works.
    [[ ${__atuin_ai_macro_bound-} ]] && bind '"\C-x\C-_Ae\a": ""'

    # If buffer is empty or just contains '?', trigger natural language mode
    if [[ -z "$READLINE_LINE" || "$READLINE_LINE" == "?" ]]; then
        READLINE_LINE=""
        READLINE_POINT=0

        local output
        output=$(atuin ai inline --hook 3>&1 1>&2 2>&3)

        if [[ $output == __atuin_ai_print__:* ]]; then
            echo "${output#__atuin_ai_print__:}"
            READLINE_LINE=""
            READLINE_POINT=0
        elif [[ $output == __atuin_ai_cancel__ ]]; then
            READLINE_LINE=""
            READLINE_POINT=0
        elif [[ $output == __atuin_ai_execute__:* ]]; then
            # Execute the command immediately
            output=${output#__atuin_ai_execute__:}
            if [[ ${BLE_ATTACHED-} ]]; then
                ble-edit/content/reset-and-check-dirty "$output"
                ble/widget/accept-line
                READLINE_LINE=""
                READLINE_POINT=0
            else
                READLINE_LINE=$output
                READLINE_POINT=${#READLINE_LINE}
                # Rebind the trailing macro keyseq so readline accepts the
                # line after this function returns.  Without the macro binding
                # (Bash <= 4.2) there is no way to invoke accept-line, so the
                # command is left in the buffer for the user to run.
                [[ ${__atuin_ai_macro_bound-} ]] && bind '"\C-x\C-_Ae\a": accept-line'
            fi
        elif [[ $output == __atuin_ai_insert__:* ]]; then
            # Insert the command for editing
            READLINE_LINE=${output#__atuin_ai_insert__:}
            READLINE_POINT=${#READLINE_LINE}
        elif [[ -n $output ]]; then
            # Default: insert for editing
            READLINE_LINE=$output
            READLINE_POINT=${#READLINE_LINE}
        fi
    else
        # Not at empty prompt, just insert the question mark
        READLINE_LINE="${READLINE_LINE:0:READLINE_POINT}?${READLINE_LINE:READLINE_POINT}"
        ((READLINE_POINT++))
    fi
}

# Set up keybindings
#
# Readline offers no way to call `accept-line' from a shell function, so we
# use the same trick as atuin.bash's enter_accept support: "?" is bound to a
# two-part string macro.  The first part (\C-x\C-_Aq\a) runs the shell
# function via `bind -x'; the second part (\C-x\C-_Ae\a) is dynamically
# rebound by the function to `accept-line' (execute) or "" (everything else),
# and readline processes it after the function returns.  The \C-x\C-_A...\a
# namespace matches atuin.bash's intermediate key sequences; the letter
# payloads (q, e) avoid colliding with its numeric ones.
#
# `bind -x' cannot bind key sequences longer than two bytes in Bash <= 4.2,
# so there we keep the direct binding and "execute" degrades to inserting
# the command for the user to run.
if ((BASH_VERSINFO[0] >= 5 || BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 3)); then
    __atuin_ai_macro_bound=1
    bind '"\C-x\C-_Ae\a": ""'
    bind -x '"\C-x\C-_Aq\a": _atuin_ai_question_mark'
    bind '"?": "\C-x\C-_Aq\a\C-x\C-_Ae\a"'
else
    bind -x '"?": _atuin_ai_question_mark'
fi
"#
    .trim()
}

/// Generate the fish integration function - pure function for easy testing
pub fn generate_fish_integration() -> &'static str {
    r#"
# Question mark at start of line - natural language mode
function _atuin_ai_question_mark
    set -l buf (commandline -b)

    # If buffer is empty or just contains '?', trigger natural language mode
    if test -z "$buf" -o "$buf" = "?"
        commandline -r ""

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
    .trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_zsh_integration() {
        let result = generate_zsh_integration();
        assert!(result.contains("self-atuin-ai-question-mark"));
        assert!(result.contains("bindkey"));
        assert!(result.contains("atuin ai inline --hook"));
        assert!(result.contains("__atuin_ai_print__"));
        assert!(result.contains("__atuin_ai_cancel__"));
        assert!(result.contains("__atuin_ai_execute__"));
        assert!(result.contains("__atuin_ai_insert__"));
        assert!(result.contains("zle self-insert"));
    }

    #[test]
    fn test_generate_bash_integration() {
        let result = generate_bash_integration();
        assert!(result.contains("_atuin_ai_question_mark"));
        assert!(result.contains("bind"));
        assert!(result.contains("READLINE_LINE"));
        assert!(result.contains("atuin ai inline --hook"));
        assert!(result.contains("__atuin_ai_print__"));
        assert!(result.contains("__atuin_ai_cancel__"));
        assert!(result.contains("__atuin_ai_execute__"));
        assert!(result.contains("__atuin_ai_insert__"));
        // Execute mode works by rebinding the trailing macro keyseq to
        // accept-line, with a direct widget call under ble.sh.
        assert!(result.contains(r#"bind '"\C-x\C-_Ae\a": accept-line'"#));
        assert!(result.contains(r#"bind '"?": "\C-x\C-_Aq\a\C-x\C-_Ae\a"'"#));
        assert!(result.contains("ble/widget/accept-line"));
    }

    #[test]
    fn test_generate_fish_integration() {
        let result = generate_fish_integration();
        assert!(result.contains("_atuin_ai_question_mark"));
        assert!(result.contains("bind"));
        assert!(result.contains("commandline"));
        assert!(result.contains("atuin ai inline --hook"));
        assert!(result.contains("__atuin_ai_print__"));
        assert!(result.contains("__atuin_ai_cancel__"));
        assert!(result.contains("__atuin_ai_execute__"));
        assert!(result.contains("__atuin_ai_insert__"));
    }
}
