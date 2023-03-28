# Source this in your ~/.config/nushell/config.nu
let-env ATUIN_SESSION = (atuin uuid)

# Magic token to make sure we don't record commands run by keybindings
let ATUIN_KEYBINDING_TOKEN = $"# (random uuid)"

let _atuin_pre_execution = {||
    let cmd = (commandline)
    if ($cmd | is-empty) {
        return
    }
    if not ($cmd | str starts-with $ATUIN_KEYBINDING_TOKEN) {
        let-env ATUIN_HISTORY_ID = (atuin history start -- $cmd)
    }
}

let _atuin_pre_prompt = {||
    let last_exit = $env.LAST_EXIT_CODE
    if 'ATUIN_HISTORY_ID' not-in $env {
        return
    }
    with-env { RUST_LOG: error } {
        atuin history end --exit $last_exit -- $env.ATUIN_HISTORY_ID | null
    }
}

def _atuin_search_cmd [...flags: string] {
    [
        $ATUIN_KEYBINDING_TOKEN,
        ([
            `commandline (RUST_LOG=error run-external --redirect-stderr atuin search`,
            ($flags | append [--interactive, --] | each {|e| $'"($e)"'}),
            `(commandline) | complete | $in.stderr | str substring ',-1')`,
        ] | flatten | str join ' '),
    ] | str join "\n"
}

let-env config = (
    $env.config | upsert hooks (
        $env.config.hooks
        | upsert pre_execution ($env.config.hooks.pre_execution | append $_atuin_pre_execution)
        | upsert pre_prompt ($env.config.hooks.pre_prompt | append $_atuin_pre_prompt)
    )
)
