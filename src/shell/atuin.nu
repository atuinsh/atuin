# Default prompt for Nushell.
def __atuin_prompt [] {
    let git = $'(do -i {git rev-parse --abbrev-ref HEAD} | str trim)'
    let git = (if ($git | str length) == 0 {
        ''
    } {
        build-string (char lparen) (ansi cb) $git (ansi reset) (char rparen)
    })
    build-string (ansi gb) (pwd) (ansi reset) $git '> '
}

# Hook to add new entries to the database.
def __atuin_hook [] {
    echo command took $CMD_DURATION_MS
}

# Initialize hook.
let-env PROMPT_STRING = (
    let prompt = (if ($nu.env | select PROMPT_STRING | empty?) {
        if ($nu.config | select prompt | empty?) { '__atuin_prompt' } { $nu.config.prompt }
    } { $nu.env.PROMPT_STRING });

    if ($prompt | str contains '__atuin_hook') { $prompt } { $'__atuin_hook;($prompt)' }
)
