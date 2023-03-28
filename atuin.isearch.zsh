#!/usr/bin/env zsh

typeset -g ATUIN_ISEARCH_N
typeset -g ATUIN_ISEARCH_MATCH
typeset -g ATUIN_ISEARCH_DURATION
typeset -g ATUIN_ISEARCH_DIR
typeset -g ATUIN_ISEARCH_LOCATION
typeset -g ATUIN_ISEARCH_TIME
typeset -g ATUIN_ISEARCH_MATCH_END

typeset -g ATUIN_ISEARCH_LAST_QUERY=""
typeset -g ATUIN_ISEARCH_LAST_N=""
typeset -g ATUIN_ISEARCH_MSG=""
typeset -g ATUIN_ISEARCH_MODES=("prefix" "fuzzy" "full-text" "skim")
typeset -g ATUIN_ISEARCH_MODE_INDEX=1
typeset -g ATUIN_ISEARCH_MODE="prefix"

typeset -g ATUIN_ISEARCH_DISPLAY_FMT='$ATUIN_ISEARCH_MSG\\nDuration: $ATUIN_ISEARCH_DURATION\\n$ATUIN_ISEARCH_TIME\\n$ATUIN_ISEARCH_LOCATION: $ATUIN_ISEARCH_DIR'


_atuin_isearch_atuin_search() {
    atuin search --limit 1 --offset "${ATUIN_ISEARCH_N}" --search-mode "$ATUIN_ISEARCH_MODE" \
        -f "{duration}:{directory}:{user}@{host}:{time}:{command}" -- "${(b)BUFFER}"
}

_atuin_isearch_query () {
    ATUIN_ISEARCH_MSG=""
    if [[ -z $BUFFER ]]; then
       ATUIN_ISEARCH_MATCH=""
       return
    fi

    local new_query="$BUFFER"
    if [[ "$new_query" != "$ATUIN_ISEARCH_LAST_QUERY" ]]; then
        ATUIN_ISEARCH_N=0
        ATUIN_ISEARCH_LAST_N=0
    fi


    local result
    result=$( _atuin_isearch_atuin_search)
    rc=$?

    if [[ "$ATUIN_ISEARCH_N" > "$ATUIN_ISEARCH_LAST_N" ]]; then
        if [[ $rc -ne 0 ]]; then
            ATUIN_ISEARCH_N=$(( ATUIN_ISEARCH_N - 1 ))
            ATUIN_ISEARCH_MSG="\nNo more results"
            return
        fi
    fi
    if [[ $rc -ne 0 ]]; then
        ATUIN_ISEARCH_MSG="\nNo results"
        ATUIN_ISEARCH_MATCH=""
        return
    fi

    ATUIN_ISEARCH_LAST_QUERY=$new_query
    ATUIN_ISEARCH_LAST_N=$ATUIN_ISEARCH_N

    local parts=(${(s/:/)result})
    ATUIN_ISEARCH_DURATION=${parts[1]}
    ATUIN_ISEARCH_DIR=${parts[2]}
    ATUIN_ISEARCH_LOCATION=${parts[3]}
    ATUIN_ISEARCH_TIME=${parts[4]}:${parts[5]}:${parts[6]}
    ATUIN_ISEARCH_MATCH=${(j.:.)parts:6}
}

_atuin_isearch_display () {
    local top_bit="atuin-isearch ($ATUIN_ISEARCH_MODE) ${ATUIN_ISEARCH_N}: "
    if [[ -z ${ATUIN_ISEARCH_MATCH} ]]; then
        # echo to get the newlines
        POSTDISPLAY=$(echo $ATUIN_ISEARCH_MSG)
        PREDISPLAY="
$top_bit"
    else
        local qbuffer="${(b)BUFFER}"
        qbuffer="${${qbuffer//\\\*/*}//\\\?/?}"
        local match_len="${#ATUIN_ISEARCH_MATCH}"
        local prefix="${ATUIN_ISEARCH_MATCH%%${~qbuffer}*}"
        local prefix_len="${#prefix}"
        local suffix_len="${#${ATUIN_ISEARCH_MATCH:${prefix_len}}##${~qbuffer}}"
        local match_end=$(( $match_len - $suffix_len ))
        ATUIN_ISEARCH_MATCH_END=${match_end}
        region_highlight=("P${prefix_len} ${match_end} underline")

        # TODO is there a better way to do this?
        POSTDISPLAY=$(eval "echo $ATUIN_ISEARCH_DISPLAY_FMT")
        PREDISPLAY="${ATUIN_ISEARCH_MATCH}
$top_bit"
    fi
}

_atuin-isearch-mode-rotate() {
    # increment and wrap
    ATUIN_ISEARCH_MODE_INDEX=$(( ($ATUIN_ISEARCH_MODE_INDEX % ${#ATUIN_ISEARCH_MODES[@]}) + 1))
    ATUIN_ISEARCH_MODE=${ATUIN_ISEARCH_MODES[$ATUIN_ISEARCH_MODE_INDEX]}
}

_atuin-isearch-up () {
    ATUIN_ISEARCH_N=$(( $ATUIN_ISEARCH_N + 1 ))
}

_atuin-isearch-down () {
    if (( $ATUIN_ISEARCH_N > 1 )); then
        ATUIN_ISEARCH_N=$(( $ATUIN_ISEARCH_N - 1 ))
    fi
}

zle -N self-insert-atuin-isearch

_atuin_line_redraw () {
    _atuin_isearch_query
    _atuin_isearch_display
}

_atuin_end_of_line () {
    # find first new line in $BUFFER and set $CURSOR to the position before it
    local new_line_pos=${${BUFFER%%$'\n'*}##$'\n'}
    CURSOR=${#new_line_pos}
}

_atuin-isearch () {
    local old_buffer=${BUFFER}
    local old_cursor=${CURSOR}
    ATUIN_ISEARCH_N=0

    zle -K atuin-isearch
    zle -N zle-line-pre-redraw _atuin_line_redraw
    _atuin_isearch_query
    _atuin_isearch_display
    zle recursive-edit
    local stat=$?
    zle -D zle-line-pre-redraw

    zle -K main
    PREDISPLAY=""
    POSTDISPLAY=""
    region_highlight=()

    if (( stat )); then
        BUFFER=${old_buffer}
        CURSOR=${old_cursor}
    else
        if [[ -n ${ATUIN_ISEARCH_MATCH} ]]; then
            BUFFER="${ATUIN_ISEARCH_MATCH}"
            CURSOR="${ATUIN_ISEARCH_MATCH_END}"
        fi
    fi

    return 0
}

zle -N _atuin-isearch-up
zle -N _atuin-isearch-down
zle -N _atuin-isearch
zle -N _atuin-isearch-mode-rotate
zle -N _atuin_end_of_line

bindkey -N atuin-isearch main
bindkey -M atuin-isearch '' _atuin-isearch-mode-rotate
bindkey -M atuin-isearch '' _atuin-isearch-up
bindkey -M atuin-isearch '' _atuin-isearch-down
bindkey -M atuin-isearch '' send-break
bindkey -M atuin-isearch '' _atuin_end_of_line
bindkey '^r' _atuin-isearch
