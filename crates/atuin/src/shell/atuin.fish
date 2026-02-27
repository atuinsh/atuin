if not set -q ATUIN_SESSION; or test "$ATUIN_SHLVL" != "$SHLVL"
    set -gx ATUIN_SESSION (atuin uuid)
    set -gx ATUIN_SHLVL $SHLVL
end
set --erase ATUIN_HISTORY_ID

function _atuin_preexec --on-event fish_preexec
    if not test -n "$fish_private_mode"
        set -g ATUIN_HISTORY_ID (atuin history start -- "$argv[1]" 2>/dev/null)
    end
end

function _atuin_postexec --on-event fish_postexec
    set -l s $status

    if test -n "$ATUIN_HISTORY_ID"
        ATUIN_LOG=error atuin history end --exit $s -- $ATUIN_HISTORY_ID &>/dev/null &
        disown
    end

    set --erase ATUIN_HISTORY_ID
end

# Check if tmux popup is available (tmux >= 3.2)
function _atuin_tmux_popup_check
    if not test -n "$TMUX"
        echo 0
        return
    end

    if test "$ATUIN_TMUX_POPUP" = false
        echo 0
        return
    end

    set -l tmux_version (tmux -V 2>/dev/null | string match -r '\d+\.\d+')
    if not test -n "$tmux_version"
        echo 0
        return
    end

    set -l parts (string split '.' $tmux_version)
    set -l m1 $parts[1]
    set -l m2 0
    if test (count $parts) -ge 2
        set m2 $parts[2]
    end

    if not string match -rq '^[0-9]+$' -- "$m1"
        echo 0
        return
    end

    if not string match -rq '^[0-9]+$' -- "$m2"
        set m2 0
    end

    if test "$m1" -gt 3 2>/dev/null; or begin
            test "$m1" -eq 3 2>/dev/null; and test "$m2" -ge 2 2>/dev/null
        end
        echo 1
    else
        echo 0
    end
end

function _atuin_search
    set -l keymap_mode
    switch $fish_key_bindings
        case fish_vi_key_bindings
            switch $fish_bind_mode
                case default
                    set keymap_mode vim-normal
                case insert
                    set keymap_mode vim-insert
            end
        case '*'
            set keymap_mode emacs
    end

    set -l use_tmux_popup (_atuin_tmux_popup_check)

    set -l ATUIN_H
    if test "$use_tmux_popup" -eq 1
        set -l tmpdir (mktemp -d)
        if not test -d "$tmpdir"
            # if mktemp got errors
            set ATUIN_H (ATUIN_SHELL=fish ATUIN_LOG=error ATUIN_QUERY=(commandline -b) atuin search --keymap-mode=$keymap_mode $argv -i 3>&1 1>&2 2>&3 | string collect)
        else
            set -l result_file "$tmpdir/result"

            set -l query (commandline -b | string replace -a "'" "'\\''")
            set -l escaped_args ""
            for arg in $argv
                set escaped_args "$escaped_args '"(string replace -a "'" "'\\''" -- $arg)"'"
            end

            # In the popup, atuin goes to terminal, stderr goes to file
            set -l cdir (pwd)
            # Keep default value anyways
            set -l popup_width (test -n "$ATUIN_TMUX_POPUP_WIDTH" && echo "$ATUIN_TMUX_POPUP_WIDTH" || echo "80%")
            set -l popup_height (test -n "$ATUIN_TMUX_POPUP_HEIGHT" && echo "$ATUIN_TMUX_POPUP_HEIGHT" || echo "60%")
            tmux display-popup -d "$cdir" -w "$popup_width" -h "$popup_height" -E -E -- \
                sh -c "PATH='$PATH' ATUIN_SESSION='$ATUIN_SESSION' ATUIN_SHELL=fish ATUIN_LOG=error ATUIN_QUERY='$query' atuin search --keymap-mode=$keymap_mode$escaped_args -i 2>'$result_file'"

            if test -f "$result_file"
                set ATUIN_H (cat "$result_file" | string collect)
            end

            command rm -rf "$tmpdir"
        end
    else
        # In fish 3.4 and above we can use `"$(some command)"` to keep multiple lines separate;
        # but to support fish 3.3 we need to use `(some command | string collect)`.
        # https://fishshell.com/docs/current/relnotes.html#id24 (fish 3.4 "Notable improvements and fixes")
        set ATUIN_H (ATUIN_SHELL=fish ATUIN_LOG=error ATUIN_QUERY=(commandline -b) atuin search --keymap-mode=$keymap_mode $argv -i 3>&1 1>&2 2>&3 | string collect)
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
