# Source this in your ~/.config/nushell/config.nu
# minimum supported version = 0.93.0
module compat {
  export def --wrapped "random uuid -v 7" [...rest] { atuin uuid }
}
use (if not (
    (version).major > 0 or
    (version).minor >= 103
) { "compat" }) *

if 'ATUIN_SESSION' not-in $env or ('ATUIN_SHLVL' not-in $env) or ($env.ATUIN_SHLVL != ($env.SHLVL? | default "")) {
    $env.ATUIN_SESSION = (random uuid -v 7 | str replace -a "-" "")
    $env.ATUIN_SHLVL = ($env.SHLVL? | default "")
}
hide-env -i ATUIN_HISTORY_ID

def _atuin_osc133_command_executed [] {
    if 'ATUIN_PTY_PROXY_ACTIVE' not-in $env {
        return
    }
    if 'ATUIN_HISTORY_ID' not-in $env or ($env.ATUIN_HISTORY_ID | is-empty) {
        return
    }

    print -n $"(char -u '1b')]133;C(char bel)"
}

def _atuin_osc133_command_finished [exit_code: int] {
    if 'ATUIN_PTY_PROXY_ACTIVE' not-in $env {
        return
    }
    if 'ATUIN_HISTORY_ID' not-in $env or ($env.ATUIN_HISTORY_ID | is-empty) {
        return
    }

    print -n $"(char -u '1b')]133;D;($exit_code);history_id=($env.ATUIN_HISTORY_ID);session_id=($env.ATUIN_SESSION)(char bel)"
}

# Magic token to make sure we don't record commands run by keybindings
let ATUIN_KEYBINDING_TOKEN = $"# (random uuid)"

def _atuin_search [_token: string, shell_up_key_binding: bool] {
    let flags = if $shell_up_key_binding { ["--shell-up-key-binding"] } else { [] }
    let search_env = if (version).minor >= 106 or (version).major > 0 {
        { ATUIN_QUERY: (commandline), ATUIN_SHELL: nu }
    } else {
        { ATUIN_QUERY: (commandline) }
    }
    with-env $search_env {
        run-external atuin search ...$flags "--interactive" e>| str trim
    }
}

def _atuin_search_cmd [shell_up_key_binding: bool = false] {
    # Older Nushell versions reject unsupported flags even in skipped branches.
    let edit_cmd = if (version).minor >= 106 or (version).major > 0 {
        'do { if ($in | str starts-with "__atuin_accept__:") { commandline edit --accept ($in | str replace "__atuin_accept__:" "") } else { commandline edit $in } }'
    } else {
        'do { commandline edit $in }'
    }

    $'_atuin_search "($ATUIN_KEYBINDING_TOKEN)" ($shell_up_key_binding) | ($edit_cmd)'
}

let _atuin_pre_execution = {||
    if ($nu | get history-enabled?) == false {
        return
    }
    let cmd = (commandline)
    if ($cmd | is-empty) {
        return
    }
    if $cmd not-in [(_atuin_search_cmd), (_atuin_search_cmd true)] {
        $env.ATUIN_HISTORY_ID = (with-env { ATUIN_SHELL: nu } {
            atuin history start --hook -- $cmd | complete | get stdout | str trim
        })
        _atuin_osc133_command_executed
    }
}

let _atuin_pre_prompt = {||
    let last_exit = $env.LAST_EXIT_CODE
    if 'ATUIN_HISTORY_ID' not-in $env {
        return
    }
    _atuin_osc133_command_finished $last_exit
    if (version).minor >= 104 or (version).major > 0 {
        job spawn {
            ^atuin history end --hook $'--exit=($env.LAST_EXIT_CODE)' -- $env.ATUIN_HISTORY_ID | complete
        } | ignore
    } else {
        do { atuin history end --hook $'--exit=($last_exit)' -- $env.ATUIN_HISTORY_ID } | complete
    }
    hide-env -i ATUIN_HISTORY_ID
}

$env.config = ($env | default {} config).config
$env.config = ($env.config | default {} hooks)
$env.config = (
    $env.config | upsert hooks (
        $env.config.hooks
        | upsert pre_execution (
            $env.config.hooks | get pre_execution? | default [] | append $_atuin_pre_execution)
        | upsert pre_prompt (
            $env.config.hooks | get pre_prompt? | default [] | append $_atuin_pre_prompt)
    )
)

$env.config = ($env.config | default [] keybindings)

if (version).minor >= 104 or (version).major > 0 {
    with-env { ATUIN_SHELL: nu } {
        job spawn {
            atuin __internal prepare-search-index | complete
        } | ignore
    }
}
