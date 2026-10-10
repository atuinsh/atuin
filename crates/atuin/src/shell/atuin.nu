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

if '__atuin_pty_proxy' not-in $env {
    # The pty-proxy preamble also sets this variable, but make sure it's set here,
    # so a manually started proxy still functions when `pty_proxy.enabled` is false.
    # We are using a record rather than a plain string so it doesn't get exported
    # to child processes -- we want each child shell to perform its own detection.
    $env.__atuin_pty_proxy = { owns_tty: (if 'ATUIN_PTY_PROXY_ACTIVE' in $env {
        do {
            let atuin_pty_proxy_check = (do -i { atuin __internal pty-proxy-active } | complete)
            $atuin_pty_proxy_check.exit_code == 0 and ($atuin_pty_proxy_check.stdout | str trim) == "1"
        }
    } else { false }) }
}

def _atuin_mark_output_start [] {
    if not ($env.__atuin_pty_proxy?.owns_tty? | default false) {
        return
    }

    if $env.__atuin_pty_proxy.needs_osc133_reset? == true {
        $env.__atuin_pty_proxy.needs_osc133_reset = false
        # Old pty-proxy will reset an in-progress capture on a `B` marker.
        # Always reset, even if there's no history ID, to avoid capturing a
        # filtered command.
        print -n $"(char -u '1b')]133;B(char bel)"
    }

    if 'ATUIN_HISTORY_ID' not-in $env or ($env.ATUIN_HISTORY_ID | is-empty) {
        return
    }

    if $env.ATUIN_PTY_PROXY_ACTIVE? == 1 {
        # Current pty-proxy is an older version that expects OSC 133; new
        # pty-proxy sets ATUIN_PTY_PROXY_ACTIVE to 2.
        print -n $"(char -u '1b')]133;C(char bel)"
    } else {
        print -n $"(char -u '1b')]18188735;C;($env.ATUIN_HISTORY_ID)(char bel)"
    }
}

def _atuin_mark_output_end [exit_code: int] {
    if not ($env.__atuin_pty_proxy?.owns_tty? | default false) {
        return
    }
    if 'ATUIN_HISTORY_ID' not-in $env or ($env.ATUIN_HISTORY_ID | is-empty) {
        return
    }

    if $env.ATUIN_PTY_PROXY_ACTIVE? == 1 {
        print -n $"(char -u '1b')]133;D;($exit_code);history_id=($env.ATUIN_HISTORY_ID)(char bel)"
    } else {
        print -n $"(char -u '1b')]18188735;D;($env.ATUIN_HISTORY_ID)(char bel)"
    }
}

# Magic token to make sure we don't record commands run by keybindings
let ATUIN_KEYBINDING_TOKEN = $"# (random uuid)"

let _atuin_pre_execution = {||
    if ($nu | get history-enabled?) == false {
        return
    }
    let cmd = (commandline)
    if ($cmd | is-empty) {
        return
    }
    if not ($cmd | str starts-with $ATUIN_KEYBINDING_TOKEN) {
        $env.ATUIN_HISTORY_ID = (with-env { ATUIN_SHELL: nu } {
            atuin history start --hook -- $cmd | complete | get stdout | str trim
        })
        _atuin_mark_output_start
    }
}

let _atuin_pre_prompt = {||
    let last_exit = $env.LAST_EXIT_CODE
    if 'ATUIN_HISTORY_ID' not-in $env {
        return
    }
    _atuin_mark_output_end $last_exit
    if (version).minor >= 104 or (version).major > 0 {
        job spawn {
            ^atuin history end --hook $'--exit=($env.LAST_EXIT_CODE)' -- $env.ATUIN_HISTORY_ID | complete
        } | ignore
    } else {
        do { atuin history end --hook $'--exit=($last_exit)' -- $env.ATUIN_HISTORY_ID } | complete
    }
    hide-env -i ATUIN_HISTORY_ID
}

def _atuin_search_cmd [...flags: string] {
    if (version).minor >= 106 or (version).major > 0 {
        [
            $ATUIN_KEYBINDING_TOKEN,
            ([
                `with-env { ATUIN_QUERY: (commandline), ATUIN_SHELL: nu } {`,
                    ([
                        'let output = (run-external atuin search',
                        ($flags | append [--interactive] | each {|e| $'"($e)"'}),
                        'e>| str trim)',
                    ] | flatten | str join ' '),
                    # An empty result (return-original) keeps the command line as typed.
                    'if ($output | is-empty) {',
                    '} else if ($output | str starts-with "__atuin_accept__:") {',
                    'commandline edit --accept ($output | str replace "__atuin_accept__:" "")',
                    '} else {',
                    'commandline edit $output',
                    '}',
                `}`,
            ] | flatten | str join "\n"),
        ]
    } else {
        [
            $ATUIN_KEYBINDING_TOKEN,
            ([
                `with-env { ATUIN_QUERY: (commandline) } {`,
                    'commandline edit',
                    '(run-external atuin search',
                        ($flags | append [--interactive] | each {|e| $'"($e)"'}),
                    ' e>| str trim)',
                `}`,
            ] | flatten | str join ' '),
        ]
    } | str join "\n"
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

def _atuin_helix_edit_modes_if_supported [modes: list<string> = [helix_normal, helix_insert]] {
    if ((version).major > 0 or (version).minor >= 115) { $modes } else { [] }
}

if (version).minor >= 104 or (version).major > 0 {
    with-env { ATUIN_SHELL: nu } {
        job spawn {
            atuin __internal prepare-search-index | complete
        } | ignore
    }
}

if $env.__atuin_pty_proxy?.owns_tty? == true and $env.ATUIN_PTY_PROXY_ACTIVE? == 1 {
    # We're running in an old pty-proxy that expects OSC 133 markers. The outer
    # shell may have already sent a `C` marker, causing the proxy to start
    # capturing output. We need to clear this state before the first command's
    # output starts, or else the prompt and command itself will be erroneously
    # included in the output.
    $env.__atuin_pty_proxy.needs_osc133_reset = true
}
