if not set -q ATUIN_SESSION; or test "$ATUIN_SHLVL" != "$SHLVL"
    set -gx ATUIN_SESSION (atuin uuid)
    set -gx ATUIN_SHLVL $SHLVL
end
set --erase ATUIN_HISTORY_ID

function _atuin_osc133_command_executed
    set -q ATUIN_PTY_PROXY_ACTIVE; or return
    test -n "$ATUIN_HISTORY_ID"; or return

    printf '\033]133;C\a'
end

function _atuin_osc133_command_finished --argument-names exit_code
    set -q ATUIN_PTY_PROXY_ACTIVE; or return
    test -n "$ATUIN_HISTORY_ID"; or return

    printf '\033]133;D;%s;history_id=%s;session_id=%s\a' "$exit_code" "$ATUIN_HISTORY_ID" "$ATUIN_SESSION"
end

function _atuin_preexec --on-event fish_preexec
    if not test -n "$fish_private_mode"
        set -g ATUIN_HISTORY_ID (ATUIN_SHELL=fish atuin history start --hook -- "$argv[1]" 2>/dev/null)
        _atuin_osc133_command_executed
    end
end

function _atuin_postexec --on-event fish_postexec
    set -l s $status

    if test -n "$ATUIN_HISTORY_ID"
        _atuin_osc133_command_finished $s
        atuin history end --hook --exit $s -- $ATUIN_HISTORY_ID &>/dev/null &
        disown
    end

    set --erase ATUIN_HISTORY_ID
end

# Popup detection intentionally uses only environment markers and version checks.
# It does not inspect process ancestry; tmux wins whenever both backends qualify.
# Check whether a version meets the requested minimum.
function _atuin_version_ge
    set -l candidate_version $argv[1]
    set -l required_major $argv[2]
    set -l required_minor $argv[3]
    set -l required_patch 0
    if test (count $argv) -ge 4
        set required_patch $argv[4]
    end

    set -l parts (string split '.' -- $candidate_version)
    set -l major $parts[1]
    set -l minor 0
    set -l patch 0
    if test (count $parts) -ge 2
        set minor $parts[2]
    end
    if test (count $parts) -ge 3
        set patch $parts[3]
    end

    string match -rq '^[0-9]+$' -- "$major"; or return 1
    string match -rq '^[0-9]+$' -- "$minor"; or return 1
    string match -rq '^[0-9]+$' -- "$patch"; or return 1

    test "$major" -gt "$required_major"; and return 0
    test "$major" -lt "$required_major"; and return 1
    test "$minor" -gt "$required_minor"; and return 0
    test "$minor" -lt "$required_minor"; and return 1
    test "$patch" -ge "$required_patch"
end

# Check if tmux popup is available (tmux >= 3.2).
function _atuin_tmux_popup_check
    test -n "$TMUX"; or return 1

    set -l tmux_version (tmux -V 2>/dev/null | string match -r '\d+\.\d+')
    _atuin_version_ge "$tmux_version" 3 2
end

# Check if a Zellij popup is available (Zellij >= 0.44.1).
function _atuin_zellij_popup_check
    test -n "$ZELLIJ"; or return 1

    set -l zellij_version (zellij --version 2>/dev/null | string match -r '\d+\.\d+\.\d+')
    _atuin_version_ge "$zellij_version" 0 44 1
end

# tmux has priority when both mux environments are detected.
function _atuin_popup_backend
    test "$ATUIN_POPUP_ENABLED" = true; or return 1

    if _atuin_tmux_popup_check
        echo tmux
    else if _atuin_zellij_popup_check
        echo zellij
    else
        return 1
    end
end

function _atuin_search
    set -l keymap_mode
    switch $fish_key_bindings
        case fish_vi_key_bindings fish_hybrid_key_bindings
            switch $fish_bind_mode
                case default
                    set keymap_mode vim-normal
                case insert
                    set keymap_mode vim-insert
            end
        case '*'
            set keymap_mode emacs
    end

    set -l popup_backend (_atuin_popup_backend)

    set -l ATUIN_H
    set -l ATUIN_STATUS 0
    # No backend, or failure to prepare one, is a preflight condition. Rendering
    # inline here cannot open a second search after a popup has already run.
    if test -n "$popup_backend"
        set -l tmpdir (mktemp -d)
        if not test -d "$tmpdir"
            # if mktemp got errors
            set ATUIN_H (ATUIN_SHELL=fish ATUIN_QUERY=(commandline -b) atuin search --keymap-mode=$keymap_mode $argv -i 3>&1 1>&2 2>&3 3>&- | string collect)
            set ATUIN_STATUS $pipestatus[1]
        else
            set -l result_file "$tmpdir/result"

            set -l query (commandline -b | string replace -a "'" "'\\''")
            set -l escaped_result_file (string replace -a "'" "'\\''" -- "$result_file")
            set -l escaped_args ""
            for arg in $argv
                set escaped_args "$escaped_args '"(string replace -a "'" "'\\''" -- $arg)"'"
            end

            # --result-file carries only the selected command; diagnostics stay in the
            # popup terminal.
            set -l cdir (pwd)
            # Build the search command once; only the mux launcher differs.
            set -l popup_command "PATH='$PATH' ATUIN_SESSION='$ATUIN_SESSION' ATUIN_SHELL=fish ATUIN_QUERY='$query' atuin search --keymap-mode=$keymap_mode$escaped_args -i --result-file '$escaped_result_file'"
            set -l popup_width (test -n "$ATUIN_POPUP_WIDTH" && echo "$ATUIN_POPUP_WIDTH" || echo "80%")
            set -l popup_height (test -n "$ATUIN_POPUP_HEIGHT" && echo "$ATUIN_POPUP_HEIGHT" || echo "60%")

            # From this point, propagate launch/result failures instead of retrying inline.
            switch "$popup_backend"
                case tmux
                    tmux display-popup -d "$cdir" -w "$popup_width" -h "$popup_height" -E -E -- \
                        sh -c "$popup_command"
                    set ATUIN_STATUS $status
                case zellij
                    # Blocking makes the result available before return; an empty name keeps the
                    # launch command and query out of the pane title.
                    zellij action new-pane --floating --name= --cwd "$cdir" --width "$popup_width" \
                        --height "$popup_height" --close-on-exit --block-until-exit -- \
                        sh -c "$popup_command" >/dev/null
                    set ATUIN_STATUS $status
            end

            if test "$ATUIN_STATUS" -eq 0
                if test -f "$result_file"
                    set ATUIN_H (command cat "$result_file" | string collect)
                    set ATUIN_STATUS $pipestatus[1]
                else
                    set ATUIN_STATUS 1
                end
            end

            command rm -rf "$tmpdir"
        end
    else
        # In fish 3.4 and above we can use `"$(some command)"` to keep multiple lines separate;
        # but to support fish 3.3 we need to use `(some command | string collect)`.
        # https://fishshell.com/docs/current/relnotes.html#id24 (fish 3.4 "Notable improvements and fixes")
        set ATUIN_H (ATUIN_SHELL=fish ATUIN_QUERY=(commandline -b) atuin search --keymap-mode=$keymap_mode $argv -i 3>&1 1>&2 2>&3 3>&- | string collect)
        set ATUIN_STATUS $pipestatus[1]
    end

    if test "$ATUIN_STATUS" -ne 0
        test -n "$ATUIN_H"; and printf '%s\n' "$ATUIN_H" >&2
        commandline -f repaint
        return "$ATUIN_STATUS"
    end

    set ATUIN_H (string trim -- $ATUIN_H | string collect) # trim whitespace

    if test -n "$ATUIN_H"
        if string match --quiet '__atuin_accept__:*' "$ATUIN_H"
            set -l ATUIN_HIST (string replace "__atuin_accept__:" "" -- "$ATUIN_H" | string collect)
            commandline -r "$ATUIN_HIST"
            commandline -f repaint
            commandline -f execute
            return
        else
            commandline -r "$ATUIN_H"
        end
    end

    commandline -f repaint
end

function _atuin_bind_up
    # Fallback to fish's builtin up-or-search if we're in search or paging mode
    if commandline --search-mode; or commandline --paging-mode
        up-or-search
        return
    end

    # Only invoke atuin if we're on the top line of the command
    set -l lineno (commandline --line)

    switch $lineno
        case 1
            _atuin_search --shell-up-key-binding
        case '*'
            up-or-search
    end
end

ATUIN_SHELL=fish atuin __internal prepare-search-index &>/dev/null &
disown 2>/dev/null
