# Source this in your ~/.config/nushell/config.nu
let-env ATUIN_SESSION = (atuin uuid)

let _atuin_pre_execution = {||
    let-env ATUIN_HISTORY_ID = (atuin history start -- (commandline))
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
        `commandline (sh -c 'RUST_LOG=error atuin search `,
        $flags,
        ` -i -- "$0" 3>&1 1>&2 2>&3' (commandline))`,
    ] | flatten | str join ''
}

let-env config = (
    $env.config | upsert hooks (
        $env.config.hooks
        | upsert pre_execution ($env.config.hooks.pre_execution | append $_atuin_pre_execution)
        | upsert pre_prompt ($env.config.hooks.pre_prompt | append $_atuin_pre_prompt)
    )
)
