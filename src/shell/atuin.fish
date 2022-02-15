set -gx ATUIN_SESSION (atuin uuid)

function _atuin_preexec --on-event fish_preexec
    set -gx ATUIN_HISTORY_ID (atuin history start "$argv[1]")
end

function _atuin_postexec --on-event fish_postexec
    set s $status
    if test -n "$ATUIN_HISTORY_ID"
        RUST_LOG=error atuin history end $ATUIN_HISTORY_ID --exit $s &
        disown
    end
end

function _atuin_search
    set h (RUST_LOG=error atuin search -i (commandline -b) 3>&1 1>&2 2>&3)
    commandline -f repaint
    if test -n "$h"
        commandline -r $h
    end
end

if test -z $ATUIN_NOBIND
    bind \cr _atuin_search
    bind -k up _atuin_search
    bind \eOA _atuin_search
    bind \e\[A _atuin_search

    if bind -M insert > /dev/null 2>&1
        bind -M insert \cr _atuin_search
        bind -M insert -k up _atuin_search
        bind -M insert \eOA _atuin_search
        bind -M insert \e\[A _atuin_search
    end
end
