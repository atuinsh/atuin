# Source this in your ~/.config/nushell/config.nu
$env.ATUIN_SESSION = (atuin uuid)
hide-env -i ATUIN_HISTORY_ID

# Magic token to make sure we don't record commands run by keybindings
let ATUIN_KEYBINDING_TOKEN = $"# (random uuid)"

let _atuin_pre_execution = {||
    if ($nu | get -i history-enabled) == false {
        return
    }
    let cmd = (commandline)
    if ($cmd | is-empty) {
        return
    }
    if not ($cmd | str starts-with $ATUIN_KEYBINDING_TOKEN) {
        $env.ATUIN_HISTORY_ID = (atuin history start -- $cmd)
    }
}

let _atuin_pre_prompt = {||
    let last_exit = $env.LAST_EXIT_CODE
    if 'ATUIN_HISTORY_ID' not-in $env {
        return
    }
    with-env { ATUIN_LOG: error } {
        do { atuin history end $'--exit=($last_exit)' -- $env.ATUIN_HISTORY_ID } | complete

    }
    hide-env ATUIN_HISTORY_ID
}

def _atuin_search_cmd [...flags: string] {
    let nu_version = ($env.NU_VERSION | split row '.' | each { || into int })
    [
        $ATUIN_KEYBINDING_TOKEN,
        ([
            `with-env { ATUIN_LOG: error, ATUIN_QUERY: (commandline) } {`,
                (if $nu_version.0 <= 0 and $nu_version.1 <= 90 { 'commandline' } else { 'commandline edit' }),
                (if $nu_version.1 >= 92 { '(run-external atuin search' } else { '(run-external --redirect-stderr atuin search' }),
                    ($flags | append [--interactive] | each {|e| $'"($e)"'}),
                (if $nu_version.1 >= 92 { ' e>| str trim)' } else {' | complete | $in.stderr | str substring ..-1)'}),
            `}`,
        ] | flatten | str join ' '),
    ] | str join "\n"
}

$env.config = ($env | default {} config).config
$env.config = ($env.config | default {} hooks)
$env.config = (
    $env.config | upsert hooks (
        $env.config.hooks
        | upsert pre_execution (
            $env.config.hooks | get -i pre_execution | default [] | append $_atuin_pre_execution)
        | upsert pre_prompt (
            $env.config.hooks | get -i pre_prompt | default [] | append $_atuin_pre_prompt)
    )
)

$env.config = ($env.config | default [] keybindings)
