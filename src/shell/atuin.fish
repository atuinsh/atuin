set -gx ATUIN_SESSION (atuin uuid)

function _atuin_preexec --on-event fish_preexec
    set -gx ATUIN_HISTORY_ID (atuin history start -- "$argv[1]")
end

function _atuin_postexec --on-event fish_postexec
    set s $status
    if test -n "$ATUIN_HISTORY_ID"
        RUST_LOG=error atuin history end --exit $s -- $ATUIN_HISTORY_ID &
        disown
    end
end

function _atuin_search
    set h (RUST_LOG=error atuin search -i -- (commandline -b) 3>&1 1>&2 2>&3)
    commandline -f repaint
    if test -n "$h"
        commandline -r $h
    end
end

function _atuin_suppress_tui
    set -gx ATUIN_SUPPRESS_TUI "true"
end

function _atuin_unsuppress_tui
    set -ge ATUIN_SUPPRESS_TUI
end

function _atuin_bind_up
    if test -z $ATUIN_SUPPRESS_TUI
        _atuin_search
    else
        up-or-search
    end
end

if test -z $ATUIN_NOBIND
    bind \cr _atuin_search
    bind -k up _atuin_bind_up
    bind \eOA _atuin_bind_up
    bind \e\[A _atuin_bind_up
    bind \t 'commandline -f complete && _atuin_suppress_tui'
    bind \e 'commandline -f cancel && _atuin_unsuppress_tui'
    bind \r 'commandline -f execute && _atuin_unsuppress_tui'
    bind \n 'commandline -f execute && _atuin_unsuppress_tui'


    if bind -M insert > /dev/null 2>&1
        bind -M insert \cr _atuin_search
        bind -M insert -k up _atuin_bind_up
        bind -M insert \eOA _atuin_bind_up
        bind -M insert \e\[A _atuin_bind_up
        bind -M insert \t 'commandline -f complete && _atuin_suppress_tui'
        bind -M insert \e 'commandline -f cancel && _atuin_unsuppress_tui'
        bind -M insert \r 'commandline -f execute && _atuin_unsuppress_tui'
        bind -M insert \n 'commandline -f execute && _atuin_unsuppress_tui'
    end
end
