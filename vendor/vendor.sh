#!/bin/sh
# shellcheck disable=SC3043
set -euf

root_dir=$(command git rev-parse --show-toplevel)
vendor_dir=vendor
script_name=$(basename "$0")

usage() {
    cat << EOF
Usage:
  $script_name add <repo-url> <ref> [name]
  $script_name update <name> <ref>
  $script_name list

<ref> specifies which branch/tag/commit to check out in the vendored
repository.

<name> is the name of the subdirectory in 'vendor/'. By default, it is
inferred from the repository URL.
EOF
}

# args: <message>
usage_error() {
    printf >&2 '%s\n' "$script_name: $1"
    printf >&2 '%s\n' "See 'vendor.sh --help' for usage information."
    exit 1
}

# args: <message>
error() {
    printf >&2 '%s\n' "$script_name: $1"
    exit 1
}

git() {
    command git -C "$root_dir" "$@"
}

# args: <url> <ref>
fetch_and_resolve() {
    git fetch -- "$1" "$2"
    if ! git rev-parse --verify --quiet 'FETCH_HEAD^{commit}'; then
        error "could not resolve '$2' in '$1'"
    fi
}

# args: <verb> <name> <ref> <commit> <url>
make_commit_msg() {
    local verb="$1"
    local name="$2"
    local ref="$3"
    local commit="$4"
    local url="$5"
    cat << EOF
vendor.sh: $verb $name at $ref

vendored-repo-name: $name
vendored-repo-commit: $commit
vendored-repo-url: $url
EOF
}

# For each vendored repository, print `<name>`, `<url>`, and `<commit>`
# separated by tabs.
get_repos() {
    set -- --topo-order --all-match --no-show-signature \
        --grep='^vendor\.sh: ' \
        --grep='^vendored-repo-name: ' \
        --grep='^vendored-repo-commit: ' \
        --grep='^vendored-repo-url: ' \
        --format='@start%n%B%n@end'
    git log "$@" | awk '
        $1 == "@start" {
            name = ""
            commit = ""
            url = ""
            next
        }
        $1 == "vendored-repo-name:" {
            name = $2
            next
        }
        $1 == "vendored-repo-commit:" {
            commit = $2
            next
        }
        $1 == "vendored-repo-url:" {
            url = $2
            next
        }
        $1 == "@end" && name != "" && commit != "" && url != "" {
            if (name in seen) next
            seen[name] = 1
            print name "\t" url "\t" commit
        }
    '
}

cmd_add() {
    if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
        usage_error "unexpected number of arguments to 'add'"
    fi

    local url="$1"
    local ref="$2"

    local name
    if [ $# -eq 3 ]; then
        name="$3"
    else
        name=${url%/}
        name=${name##*/}
        name=${name%.git}
    fi

    if [ -z "$name" ]; then
        error "could not infer a name from '$url'"
    fi
    case $name in
        */* | . | ..) error "invalid name: '$name'" ;;
    esac
    if [ -e "$vendor_dir/$name" ]; then
        error "'$vendor_dir/$name' already exists"
    fi

    local commit
    commit=$(fetch_and_resolve "$url" "$ref")
    git subtree add \
        --squash \
        --prefix="$vendor_dir/$name" \
        --message="$(make_commit_msg add "$name" "$ref" "$commit" "$url")" \
        "$commit"
}

cmd_update() {
    if [ "$#" -ne 2 ]; then
        usage_error "unexpected number of arguments to 'update'"
    fi

    local name="$1"
    local ref="$2"
    local url
    url=$(get_repos | awk -F'\t' -v n="$name" '$1 == n { print $2; exit }')
    if [ -z "$url" ]; then
        error "no vendored repository named '$name'"
    fi

    local commit
    commit=$(fetch_and_resolve "$url" "$ref")
    git subtree merge \
        --squash \
        --prefix="$vendor_dir/$name" \
        --message="$(make_commit_msg update "$name" "$ref" "$commit" "$url")" \
        "$commit"
}

cmd_list() {
    if [ "$#" -ne 0 ]; then
        usage_error "unexpected number of arguments to 'list'"
    fi
    get_repos | awk '{
        print "name: " $1
        print "url: " $2
        print "commit: " $3
    }'
}

if [ "$#" -lt 1 ]; then
    usage_error "missing subcommand"
fi
cmd=$1
shift

case "$cmd" in
    add|list|update) ;;
    -h|--help)
        usage
        exit 0
        ;;
    *)
        usage_error "unknown command"
        ;;
esac
"cmd_$cmd" "$@"
