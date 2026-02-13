pub async fn run() -> eyre::Result<()> {
    let zsh_function = generate_zsh_integration();
    println!("{}", zsh_function);
    Ok(())
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
}
