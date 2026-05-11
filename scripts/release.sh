#!/usr/bin/env bash
#
# Atuin CLI Release Script
#
# But first — make a cup of tea. Releases without tea are a crime. 🫖
#

set -euo pipefail

WORKDIR=""

cleanup_on_error() {
    if [[ $? -ne 0 && -n "$WORKDIR" ]]; then
        echo ""
        echo -e "  \033[0;31m✗\033[0m Script failed. Working directory preserved at:"
        echo -e "    \033[2m$WORKDIR\033[0m"
    fi
}
trap cleanup_on_error EXIT

# ════════════════════════════════════════════════════════════════════
# Formatting
# ════════════════════════════════════════════════════════════════════

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
MAGENTA='\033[0;35m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

info()    { echo -e "  ${BLUE}▸${NC} $*"; }
success() { echo -e "  ${GREEN}✓${NC} $*"; }
warn()    { echo -e "  ${YELLOW}⚠${NC} $*"; }
die()     { echo -e "  ${RED}✗${NC} $*" >&2; exit 1; }

step() {
    echo ""
    echo -e "  ${MAGENTA}${BOLD}━━━ $* ━━━${NC}"
    echo ""
}

confirm() {
    echo -en "  ${CYAN}▸${NC} $1 ${DIM}[y/N]${NC} "
    read -r reply
    [[ "$reply" =~ ^[Yy]$ ]]
}

# ════════════════════════════════════════════════════════════════════
# Banner
# ════════════════════════════════════════════════════════════════════

banner() {
    echo ""
    echo -e "${CYAN}"
    cat <<'BANNER'

                                       :
                                     =#*#=
                                     #*+*#.
                                    #**++*+
                                   =#*++++#.
                                   **+++++**
                                  :#++++++*#.
                                  **+++++++*#
                                 -#*+++++++*#-
                                 ##*****#####*
                             -*##########:.+####*.
                             ***********####*****+
                              :**#############*#+
                              #+----------------*-
           -*###########**+++##-----=##=-----=##=#
         .#*-==..            =*------------------*
        -#-::::::*=----------#*---------=----=--=*
       +*:::::::--:::::::::::**---+#=----===---=*
      #*::::::::=::::::::::::**-----+#*=-----+#-
    .#+::::::::=-::::::::::::**--------=*####
    #=::::::::=*:::::::::::::+*---------=#-*#=
    ##-:::-**+=+*-:::::::::::-#=--------=#=##-
    :#+**+========++=::::-=++=#+--------+#+#*
    +##++============+*#+=====*+--------+#*#
    *=*#*+==+**++++*#++*====+###--------*###*
    *=-*##=+#=-------#**===+#+---------=##*=#
    *=-=*#**+---------#*+==+#----------*##--#
    :#*- ###=---------#+*==+#=-------=###=-=#
           +=---------#+#+++#*++++*##*== .. #
   ......  +=--*++*+=-#####+-.      -+****=.
   ....... =*==    .  +      ...........
   ............::-:............................
   ............................................
   ....###....########.##.....##.####.##....##.
   ...##.##......##....##.....##..##..###...##.
   ..##...##.....##....##.....##..##..####..##.
   .##.....##....##....##.....##..##..##.##.##.
   .#########....##....##.....##..##..##..####.
   .##.....##....##....##.....##..##..##...###.
   .##.....##....##.....#######..####.##....##.
   ............................................

            ~ Magical Shell History ~
                 Release Script

BANNER
    echo -e "${NC}"
    echo ""
}

# ════════════════════════════════════════════════════════════════════
# 1. Dependency check
# ════════════════════════════════════════════════════════════════════

check_deps() {
    step "Dependencies"

    local missing=()
    local deps=(git gsed cargo gh git-cliff)

    for cmd in "${deps[@]}"; do
        if command -v "$cmd" &>/dev/null; then
            success "${BOLD}$cmd${NC} ${DIM}$(command -v "$cmd")${NC}"
        else
            echo -e "  ${RED}✗${NC} ${BOLD}$cmd${NC} not found"
            missing+=("$cmd")
        fi
    done

    if (( ${#missing[@]} )); then
        echo ""
        die "Missing required tools: ${missing[*]}\n  Install with: ${DIM}brew install ${missing[*]}${NC}"
    fi
}

# ════════════════════════════════════════════════════════════════════
# 2. Clone into working directory
# ════════════════════════════════════════════════════════════════════

setup_workdir() {
    step "Working Directory"

    WORKDIR=$(mktemp -d)
    info "Created ${DIM}$WORKDIR${NC}"

    info "Cloning atuinsh/atuin..."
    git clone --quiet git@github.com:atuinsh/atuin.git "$WORKDIR"
    cd "$WORKDIR"
    success "Repository cloned"
}

# ════════════════════════════════════════════════════════════════════
# 3. Version
# ════════════════════════════════════════════════════════════════════

detect_current_version() {
    sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml
}

get_version() {
    step "Version"

    CURRENT_VERSION=$(detect_current_version)
    info "Current version: ${BOLD}$CURRENT_VERSION${NC}"

    # Suggest the next version based on conventional commits
    local suggested
    suggested=$(git-cliff --bumped-version 2>/dev/null | sed 's/^v//' || true)
    if [[ -n "$suggested" && "$suggested" != "$CURRENT_VERSION" ]]; then
        info "Suggested next:  ${BOLD}$suggested${NC} ${DIM}(based on conventional commits)${NC}"
    fi

    echo ""

    if [[ -n "${NEW_VERSION:-}" ]]; then
        info "Using version from environment: ${BOLD}$NEW_VERSION${NC}"
    else
        echo -en "  ${CYAN}▸${NC} New version ${DIM}(without 'v' prefix)${NC}: "
        read -r NEW_VERSION
    fi

    [[ -n "$NEW_VERSION" ]] || die "Version cannot be empty"

    IS_PRERELEASE=false
    if [[ "$NEW_VERSION" == *-* ]]; then
        IS_PRERELEASE=true
        warn "Pre-release detected"
    fi

    echo ""
    info "${BOLD}$CURRENT_VERSION${NC} → ${BOLD}$NEW_VERSION${NC}"
    echo ""
    confirm "Proceed with release?" || { info "Aborted."; exit 0; }
}

# ════════════════════════════════════════════════════════════════════
# 4. Update version numbers
# ════════════════════════════════════════════════════════════════════

update_versions() {
    step "Updating Versions"

    info "Creating release branch: ${BOLD}$NEW_VERSION${NC}"
    git checkout -b "$NEW_VERSION" --quiet

    local version_pattern="${CURRENT_VERSION//./\\.}"

    info "Replacing ${DIM}$CURRENT_VERSION${NC} → ${DIM}$NEW_VERSION${NC} in Cargo.toml files..."
    find . -type f -name 'Cargo.toml' -not -path './.git/*' \
        -exec gsed -i "s/$version_pattern/$NEW_VERSION/g" {} \;

    info "Running ${DIM}cargo check${NC} to update Cargo.lock (this may take a moment)..."
    cargo check --quiet 2>&1 || cargo check

    echo ""
    info "Changed files:"
    git diff --stat | sed 's/^/    /'

    echo ""

    # Verify the workspace version was updated
    local new_ws_version
    new_ws_version=$(detect_current_version)
    if [[ "$new_ws_version" == "$NEW_VERSION" ]]; then
        success "Workspace version updated"
    else
        die "Workspace version is '$new_ws_version', expected '$NEW_VERSION'"
    fi

    # Verify we didn't break anything unexpected — show version-related
    # lines in the diff for review
    echo ""
    info "Version changes in diff:"
    git diff --unified=0 -- '*.toml' \
        | grep -E '^\+.*version' \
        | grep -v '^\+\+\+' \
        | sed 's/^/    /' || true
    echo ""

    confirm "Version changes look correct?" || { die "Aborting — fix versions manually in $WORKDIR"; }
}

# ════════════════════════════════════════════════════════════════════
# 5. Changelog
# ════════════════════════════════════════════════════════════════════

update_changelog() {
    step "Changelog"

    # cliff.toml's ignore_tags already ignores beta/alpha tags, so
    # --unreleased always gives us everything since the last stable release.
    #
    # Prereleases:  heading is ## [unreleased]  (running tally)
    # Stable:       heading is ## X.Y.Z         (versioned entry)
    local cliff_args=(--unreleased --strip all)

    if $IS_PRERELEASE; then
        info "Updating ${BOLD}Unreleased${NC} section..."
    else
        cliff_args+=(--tag "v$NEW_VERSION")
        info "Generating changelog for ${BOLD}$NEW_VERSION${NC}..."
    fi

    local new_entry
    new_entry=$(git-cliff "${cliff_args[@]}" 2>/dev/null || true)

    # Check if the entry is empty (just a heading with no content)
    if [[ -z "$new_entry" ]] || [[ "$(echo "$new_entry" | grep -c '[a-zA-Z]')" -le 1 ]]; then
        warn "No unreleased changes detected by git-cliff"
        warn "You may want to add entries manually in the editor"
        if $IS_PRERELEASE; then
            new_entry="## [unreleased]"
        else
            new_entry="## $NEW_VERSION"
        fi
    else
        local commit_count
        commit_count=$(echo "$new_entry" | grep -c '^- ' || true)
        success "Generated entry with ${BOLD}$commit_count${NC} item(s)"
    fi

    # Remove any existing [unreleased] section — we'll replace it with
    # either an updated unreleased section or a versioned one
    if grep -qi '^\## \[unreleased\]' CHANGELOG.md; then
        info "Removing old Unreleased section..."
        awk '
            /^## \[[Uu]nreleased\]/ { skip=1; next }
            /^## /                   { skip=0 }
            !skip
        ' CHANGELOG.md > CHANGELOG.md.tmp
        mv CHANGELOG.md.tmp CHANGELOG.md
    fi

    # Insert the new entry before the first existing version heading
    local insert_line
    insert_line=$(grep -n '^## ' CHANGELOG.md | head -1 | cut -d: -f1)

    if [[ -n "$insert_line" ]]; then
        {
            head -n "$((insert_line - 1))" CHANGELOG.md
            echo "$new_entry"
            echo ""
            echo ""
            tail -n "+$insert_line" CHANGELOG.md
        } > CHANGELOG.md.tmp
        mv CHANGELOG.md.tmp CHANGELOG.md
    else
        warn "No existing version headings found — appending to end"
        echo "" >> CHANGELOG.md
        echo "$new_entry" >> CHANGELOG.md
    fi

    echo ""
    info "Opening CHANGELOG.md in your editor for review..."
    info "${DIM}Verify the entry, make any edits, then save and close.${NC}"
    echo ""
    echo -en "  ${DIM}Press Enter to open editor...${NC}"
    read -r
    "${EDITOR:-${VISUAL:-vi}}" CHANGELOG.md
    success "Changelog finalized"
}

# ════════════════════════════════════════════════════════════════════
# 6. Commit and push
# ════════════════════════════════════════════════════════════════════

commit_and_push() {
    step "Commit & Push"

    git add .
    git commit --quiet -m "chore(release): prepare for release $NEW_VERSION"
    success "Committed"

    info "Pushing branch..."
    git push --quiet --set-upstream origin "$(git branch --show-current)" 2>&1
    success "Pushed to origin/${BOLD}$NEW_VERSION${NC}"
}

# ════════════════════════════════════════════════════════════════════
# 7. Pull request
# ════════════════════════════════════════════════════════════════════

extract_changelog_entry() {
    # Extract the changelog body (without the heading) for the entry we just wrote.
    # Prereleases use ## [unreleased], stable uses ## X.Y.Z
    local heading
    if $IS_PRERELEASE; then
        heading='## \[unreleased\]'
    else
        heading="## ${NEW_VERSION//./\\.}"
    fi
    awk "/^${heading}/{found=1; next} /^## /{if(found) exit} found" CHANGELOG.md
}

create_pr() {
    step "Pull Request"

    local changelog_body
    changelog_body=$(extract_changelog_entry)

    local pr_body
    pr_body="Release preparation for v${NEW_VERSION}."
    if [[ -n "$changelog_body" ]]; then
        pr_body="$(cat <<EOF
Release preparation for v${NEW_VERSION}.

## Changelog

$changelog_body
EOF
)"
    fi

    info "Creating PR..."
    PR_URL=$(gh pr create \
        --title "chore(release): prepare for release $NEW_VERSION" \
        --body "$pr_body" \
        --repo atuinsh/atuin)

    success "PR created: ${BOLD}$PR_URL${NC}"

    local pr_number
    pr_number=$(echo "$PR_URL" | grep -o '[0-9]*$')

    echo ""
    info "Waiting for PR #${BOLD}$pr_number${NC} to be merged..."
    info "${DIM}Review and merge the PR — this script will detect it automatically.${NC}"
    echo ""

    while true; do
        local state
        state=$(gh pr view "$pr_number" --repo atuinsh/atuin --json state --jq '.state' 2>/dev/null || echo "UNKNOWN")

        case "$state" in
            MERGED)
                echo ""
                success "PR #$pr_number merged!"
                break
                ;;
            CLOSED)
                echo ""
                die "PR #$pr_number was closed without merging"
                ;;
            *)
                printf "\r  ${DIM}⏳ PR #%s is %s — checking again in 5s...${NC}  " "$pr_number" "$state"
                sleep 5
                ;;
        esac
    done
}

# ════════════════════════════════════════════════════════════════════
# 8. Tag
# ════════════════════════════════════════════════════════════════════

tag_release() {
    step "Tag & Release"

    info "Switching to main and pulling..."
    git checkout main --quiet
    git pull --quiet

    info "Creating tag ${BOLD}v$NEW_VERSION${NC}"
    git tag "v$NEW_VERSION"

    info "Pushing tag..."
    git push --tags
    success "Tag ${BOLD}v$NEW_VERSION${NC} pushed — release workflow triggered"
}

# ════════════════════════════════════════════════════════════════════
# 9. Publish to crates.io
# ════════════════════════════════════════════════════════════════════

publish_crates() {
    step "Publish to crates.io"

    if $IS_PRERELEASE; then
        warn "Pre-release — skipping crates.io publish"
        return
    fi

    if ! confirm "Publish to crates.io?"; then
        info "Skipping"
        return
    fi

    local crates=(
        atuin-common
        atuin-client
        atuin-ai
        atuin-dotfiles
        atuin-history
        atuin-nucleo/matcher
        atuin-nucleo
        atuin-daemon
        atuin-kv
        atuin-scripts
        atuin-server-database
        atuin-server-postgres
        atuin-server-sqlite
        atuin-server
        atuin-pty-proxy
        atuin
    )

    for crate in "${crates[@]}"; do
        info "Publishing ${BOLD}$crate${NC}..."
        local output
        # --no-verify: skip rebuild during publish — the code already passed
        # CI, and verification fails on workspace crates whose freshly-published
        # dependencies haven't been indexed by crates.io yet
        if output=$(cd "crates/$crate" && cargo publish --no-verify 2>&1); then
            success "$crate published"
        elif echo "$output" | grep -q "already uploaded"; then
            warn "$crate already published, skipping"
        else
            echo "$output" >&2
            die "Failed to publish $crate"
        fi
    done
}

# ════════════════════════════════════════════════════════════════════
# Main
# ════════════════════════════════════════════════════════════════════

main() {
    banner
    check_deps
    setup_workdir
    get_version
    update_versions
    update_changelog
    commit_and_push
    create_pr
    tag_release
    publish_crates

    step "Done 🎉"
    success "Released ${BOLD}v$NEW_VERSION${NC}"
    echo ""
    info "Working directory: ${DIM}$WORKDIR${NC}"
    info "Clean up with:     ${DIM}rm -rf $WORKDIR${NC}"
    echo ""
}

main "$@"
