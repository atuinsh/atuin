use crate::commands::detect_shell;

pub async fn run(shell: String) -> eyre::Result<()> {
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

# Question mark at start of line - natural language mode
_atuin_ai_question_mark() {
    # If buffer is empty or just contains '?', trigger natural language mode
    if [[ -z "$BUFFER" || "$BUFFER" == "?" ]]; then
        BUFFER=""
        local output
        output=$(atuin-ai inline --natural-language 3>&1 1>&2 2>&3)

        # Clean up the inline viewport
        _atuin_ai_cleanup

        if [[ $output == __atuin_ai_cancel__ ]]; then
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
        LBUFFER="${LBUFFER}?"
    fi
}

# Set up keybindings
zle -N _atuin_ai_question_mark
bindkey '?' _atuin_ai_question_mark # Question mark
"#
    .trim()
}

/// Generate the bash integration function - pure function for easy testing
pub fn generate_bash_integration() -> &'static str {
    r#"
# Question mark at start of line - natural language mode
_atuin_ai_question_mark() {
    # If buffer is empty or just contains '?', trigger natural language mode
    if [[ -z "$READLINE_LINE" || "$READLINE_LINE" == "?" ]]; then
        READLINE_LINE=""
        READLINE_POINT=0

        local output
        output=$(atuin-ai inline --natural-language 3>&1 1>&2 2>&3)

        if [[ $output == __atuin_ai_cancel__ ]]; then
            # User cancelled, do nothing
            READLINE_LINE=""
            READLINE_POINT=0
        elif [[ $output == __atuin_ai_execute__:* ]]; then
            # Execute the command immediately
            READLINE_LINE=${output#__atuin_ai_execute__:}
            READLINE_POINT=${#READLINE_LINE}
            # Note: We can't directly execute in bash bind -x, but we can
            # use a workaround by binding to a macro that accepts the line
            bind '"\C-x\C-a": accept-line'
            bind -x '"\C-x\C-e": _atuin_ai_question_mark'
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
# Bash requires special handling: we use bind -x for the function,
# but need a two-step approach for execute mode
__atuin_ai_accept_line=""

_atuin_ai_question_mark_wrapper() {
    _atuin_ai_question_mark
    if [[ -n "$__atuin_ai_accept_line" ]]; then
        __atuin_ai_accept_line=""
    fi
}

bind -x '"?": _atuin_ai_question_mark'
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

        # Run atuin-ai inline, swapping stdout and stderr
        set -l output (atuin-ai inline --natural-language 3>&1 1>&2 2>&3 | string collect)

        if test "$output" = "__atuin_ai_cancel__"
            # User cancelled, do nothing
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
    else
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
        assert!(result.contains("_atuin_ai_question_mark"));
        assert!(result.contains("bindkey"));
        assert!(result.contains("atuin-ai inline"));
        assert!(result.contains("__atuin_ai_cancel__"));
        assert!(result.contains("__atuin_ai_execute__"));
        assert!(result.contains("__atuin_ai_insert__"));
    }

    #[test]
    fn test_generate_bash_integration() {
        let result = generate_bash_integration();
        assert!(result.contains("_atuin_ai_question_mark"));
        assert!(result.contains("bind"));
        assert!(result.contains("READLINE_LINE"));
        assert!(result.contains("atuin-ai inline"));
        assert!(result.contains("__atuin_ai_cancel__"));
        assert!(result.contains("__atuin_ai_execute__"));
        assert!(result.contains("__atuin_ai_insert__"));
    }

    #[test]
    fn test_generate_fish_integration() {
        let result = generate_fish_integration();
        assert!(result.contains("_atuin_ai_question_mark"));
        assert!(result.contains("bind"));
        assert!(result.contains("commandline"));
        assert!(result.contains("atuin-ai inline"));
        assert!(result.contains("__atuin_ai_cancel__"));
        assert!(result.contains("__atuin_ai_execute__"));
        assert!(result.contains("__atuin_ai_insert__"));
    }
}
