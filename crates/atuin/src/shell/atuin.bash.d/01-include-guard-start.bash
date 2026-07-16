# Include guard
if [[ ${__atuin_initialized-} == true ]]; then
    false
elif [[ $- != *i* ]]; then
    # Enable only in interactive shells
    false
elif ((BASH_VERSINFO[0] < 3 || BASH_VERSINFO[0] == 3 && BASH_VERSINFO[1] < 1)); then
    # Require bash >= 3.1
    [[ -t 2 ]] && printf 'atuin: requires bash >= 3.1 for the integration.\n' >&2
    false
else # (include guard) beginning of main content
#------------------------------------------------------------------------------
