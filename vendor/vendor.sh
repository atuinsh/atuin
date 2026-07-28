#!/bin/sh
# This script manages vendored repositories. See `./vendor.sh --help`.
#
# shellcheck disable=SC3043  # allow `local` extension
# shellcheck disable=SC2016  # `$n` is a jq variable
set -euf

root_dir=$(command git rev-parse --show-toplevel)
vendor_dir=vendor
db_path=$vendor_dir/.repositories.json
script_name=$(basename "$0")

usage() {
    cat << EOF
Usage:
  $vendor_dir/$script_name add <repo-url> <ref> [name]
  $vendor_dir/$script_name update <name> <ref>
  $vendor_dir/$script_name list

<ref> specifies which branch/tag/commit to check out in the vendored
repository.

<name> is the name of the subdirectory in '$vendor_dir/'. By default, it is
inferred from the repository URL.
EOF
}

# args: <message>
usage_error() {
    printf >&2 '%s\n' "$script_name: $1"
    printf >&2 '%s\n' "See '$script_name --help' for usage information."
    exit 1
}

# args: <message>
error() {
    printf >&2 '%s\n' "$script_name: $1"
    exit 1
}

# Runs `git` in the root directory; forwards all args
git() {
    command git -C "$root_dir" "$@"
}

# Runs `jq` on the database; forwards all args
jq_db() {
    jq "$@" -- "$root_dir/$db_path"
}

# args: <name>
validate_name() {
    case "$1" in
        ''|*/*|.|..) error "invalid name: '$1'" ;;
    esac
}

temp=

cleanup() {
    if [ -n "$temp" ]; then
        rm -rf -- "$temp"
    fi
}

# args: <name> <url> <ref>
sync_repo() {
    local name="$1"
    local url="$2"
    local ref="$3"

    local dir="$vendor_dir/$name"
    local status
    status=$(git status --porcelain --ignored -- "$dir")
    if [ -n "$status" ]; then
        printf '%s\n' "$status"
        error "'$dir' has uncommitted changes"
    fi

    git fetch -- "$url" "$ref"
    local commit
    if ! commit=$(git rev-parse --verify 'FETCH_HEAD^{commit}'); then
        error "could not resolve '$ref' in '$url'"
    fi

    temp=$(mktemp -d)
    mkdir -p -- "$temp/$name"

    # NOTE: `git archive` honors export-ignore and will not fetch submodules.
    # For most use cases, this is fine, but we will have to change the strategy
    # if we need a vendored repository that isn't compatible with these
    # limitations.
    git archive "$commit" | tar -C "$temp/$name" -x
    rm -rf -- "${root_dir:?}/$dir"
    mv -- "$temp/$name" "$root_dir/$vendor_dir/"

    local json
    json=$(jq_db -S --arg name "$name" --arg url "$url" \
        --arg commit "$commit" '. + {$name: {url: $url, commit: $commit}}')

    printf '%s\n' "$json" > "$root_dir/$db_path"
    git add -A -- "$dir" "$db_path"
    printf '%s\n' "$script_name: staged '$dir' at $commit"
    printf '%s\n' "$script_name: review and commit the changes"
}

cmd_add() {
    if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
        usage_error "unexpected number of arguments to 'add'"
    fi

    local url="$1"
    local ref="$2"
    local name
    if [ -n "${3+x}" ]; then
        name="$3"
    else
        name=${url%/}
        name=${name##*/}
        name=${name%.git}
    fi
    validate_name "$name"

    if [ -e "$root_dir/$vendor_dir/$name" ]; then
        error "'$vendor_dir/$name' already exists"
    fi
    sync_repo "$name" "$url" "$ref"
}

cmd_update() {
    if [ "$#" -ne 2 ]; then
        usage_error "unexpected number of arguments to 'update'"
    fi

    local name="$1"
    local ref="$2"
    validate_name "$name"

    local url
    url=$(jq_db -r --arg name "$name" '(.[$name] | objects | .url) // empty')

    if [ -z "$url" ]; then
        error "no vendored repository named '$name'"
    fi
    sync_repo "$name" "$url" "$ref"
}

cmd_list() {
    if [ "$#" -ne 0 ]; then
        usage_error "unexpected number of arguments to 'list'"
    fi
    jq_db -r 'to_entries | map(select(.value | type == "object")
        | ["name: " + .key, "url: " + .value.url, "commit: " + .value.commit]
        | join("\n")
    ) | join("\n\n")'
}

if [ "$#" -lt 1 ]; then
    usage_error "missing subcommand"
fi
cmd=$1
shift

case "$cmd" in
    add|list|update) ;;
    -h|--help|help)
        usage
        exit 0
        ;;
    *)
        usage_error "unknown command"
        ;;
esac
if ! command -v jq > /dev/null; then
    printf >&2 '%s\n' "error: missing jq"
    exit 1
fi
trap cleanup HUP INT QUIT TERM EXIT
"cmd_$cmd" "$@"
