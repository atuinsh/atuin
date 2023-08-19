#!/bin/bash

inspect() {
    local name="$1"
    local from="$2"
    local to="$3"
    local criteria="$4"

    if [ "$from" = "null" ]
    then
        open "https://sourcegraph.com/crates/$name@v$to"
        cargo vet certify --criteria "$criteria" "$name" "$to"
    else
        open "https://sourcegraph.com/crates/$name/-/compare/v$from...v$to"
        cargo vet certify --criteria "$criteria" "$name" "$from" "$to"
    fi
}

suggest_one() {
    local criteria="$1"

    suggest=$(cargo vet suggest --output-format json 2> /dev/null)
    suggestion=$(echo "$suggest" | jq ".suggest.suggest_by_criteria[\"$criteria\"][0]")

    if [ "$suggestion" = "null" ]
    then
        echo "No more crates to inspect for this criteria. Try one of the following:".
        echo "$suggest" | jq ".suggest.suggest_by_criteria | keys"
        exit 0
    fi

    name=$(echo "$suggestion" | jq -r ".name")
    from=$(echo "$suggestion" | jq -r ".suggested_diff.from")
    to=$(echo "$suggestion" | jq -r ".suggested_diff.to")

    read -r -p "Inspect $name $to? [Y]es/[N]o: " -n 1 process

    case "$process" in
        n|N)
            return 1
        ;;
        *)
            inspect "$name" "$from" "$to" "$criteria"
        ;;
    esac
}

while :
do
    if ! suggest_one $1
    then
        exit 0
    fi
done
