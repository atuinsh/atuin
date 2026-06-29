---
name: release
description: >
  Orchestrate a multi-step Atuin CLI release — version bumping, changelog
  generation, PR creation, tagging, and crates.io publishing. Invoke with
  /release or /release <version>.
disable-model-invocation: true
argument-hint: [version]
---

# Atuin CLI Release

You are orchestrating a release of the Atuin CLI. Follow the steps below
**in order**, pausing at each checkpoint for user confirmation. Do not skip
steps or combine them.

## Current State

- Workspace version: !`sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml`
- Latest tag: !`git describe --tags --abbrev=0 2>/dev/null || echo "none"`
- Suggested next version: !`git-cliff --bumped-version 2>/dev/null | sed 's/^v//' || echo "(unknown)"`

---

## Step 1 — Check Dependencies

Verify these tools are installed: `git`, `gsed`, `cargo`, `gh`, `git-cliff`.

Use `command -v` for each. If any are missing, report which ones and stop.

---

## Step 2 — Determine Version

The target version may be provided as `$ARGUMENTS`. If it's empty, use
AskUserQuestion to ask for the new version (show the current state above
for reference).

After determining the version:
- If it contains a `-` (e.g. `18.15.0-beta.1`), it is a **prerelease**.
  Note this — it affects changelog and publish behavior later.
- Show the user: `current → new` and whether it's a prerelease.
- **Checkpoint:** Ask the user to confirm before proceeding.

---

## Step 3 — Set Up Working Directory

Clone a fresh copy into a temp directory:

```bash
WORKDIR=$(mktemp -d)
git clone git@github.com:atuinsh/atuin.git "$WORKDIR"
```

Print the working directory path so the user can find it if needed.
All subsequent Bash commands run from `$WORKDIR`.

---

## Step 4 — Create Branch & Update Versions

1. Create a release branch named after the version (no `v` prefix):
   `git checkout -b <VERSION>`

2. Replace the old version with the new one in all `Cargo.toml` files.
   **Escape dots** in the old version so sed treats them literally:

   ```bash
   VERSION_PATTERN="${OLD_VERSION//./\\.}"
   find . -type f -name 'Cargo.toml' -not -path './.git/*' \
       -exec gsed -i "s/$VERSION_PATTERN/$NEW_VERSION/g" {} \;
   ```

3. Run `cargo check` to update `Cargo.lock`.

4. Show `git diff --stat` and the version-related lines from the diff:
   ```bash
   git diff --unified=0 -- '*.toml' | grep -E '^\+.*version' | grep -v '^\+\+\+'
   ```

5. Verify the workspace version was actually updated by re-reading it
   from `Cargo.toml`.

6. **Checkpoint:** Show the diff summary and ask the user to confirm the
   version changes look correct.

---

## Step 5 — Update Changelog

The changelog strategy differs for prereleases vs stable releases:

- **Prerelease:** Maintain a running `## [unreleased]` section containing
  all changes since the last stable release. Use:
  `git-cliff --unreleased --strip all`
  (cliff.toml's `ignore_tags` already ignores beta/alpha tags, so
  `--unreleased` spans back to the last stable release automatically.)

- **Stable release:** Generate a versioned entry that replaces the
  `[unreleased]` section. Use:
  `git-cliff --unreleased --tag "v<VERSION>" --strip all`

Then update `CHANGELOG.md`:

1. If an existing `## [unreleased]` or `## [Unreleased]` section exists,
   **remove it entirely** (the heading and all content up to the next
   `## ` heading).

2. Insert the new entry before the first existing `## ` version heading.

3. **Checkpoint:** Read and display the new changelog entry to the user.
   Ask if they want any edits. If so, make the requested changes using
   the Edit tool. Repeat until they're satisfied.

---

## Step 6 — Commit & Push

Stage all changes and commit:

```
chore(release): prepare for release <VERSION>
```

Push the branch with `--set-upstream origin`.

---

## Step 7 — Create PR & Wait for Merge

### Create the PR

Extract the changelog entry body (everything between the new `## ` heading
and the next one) for the PR description.

For prereleases, the heading to match is `## [unreleased]`.
For stable releases, it's `## <VERSION>` (escape dots in the awk pattern).

Create the PR:
```bash
gh pr create \
    --title "chore(release): prepare for release <VERSION>" \
    --body "<body with changelog>" \
    --repo atuinsh/atuin
```

Show the PR URL to the user.

### Wait for merge

Start a **persistent Monitor** that polls the PR status every 30 seconds.
The monitor script must:
- **Only emit output** on meaningful state changes: all checks green, PR
  merged, or PR closed. Silent polls keep the monitor quiet and avoid
  flooding notifications.
- Handle transient API errors gracefully (don't crash on a single failure)
- Exit 0 on `MERGED`, exit 1 on `CLOSED`

The rollup mixes two entry shapes: `CheckRun` entries use `status` +
`conclusion`, while `StatusContext` entries use `state`. A check counts
as "passing" when it's in a terminal state with a non-failing outcome.
Treat `SUCCESS`, `SKIPPED`, and `NEUTRAL` as passing — some release
workflows (e.g. `announce`, `build-global-artifacts`) are conditional
and report `SKIPPED` on non-tag events, which is expected, not a
failure.

Example monitor script (substitute the actual PR number):
```bash
checks_passed=false
while true; do
  json=$(gh pr view PR_NUM --repo atuinsh/atuin --json state,statusCheckRollup 2>/dev/null) || { sleep 30; continue; }
  state=$(echo "$json" | jq -r '.state')
  case "$state" in
    MERGED) echo "PR #PR_NUM has been merged!"; exit 0 ;;
    CLOSED) echo "PR #PR_NUM was closed without merging."; exit 1 ;;
  esac
  # Only notify once when all checks reach a terminal passing state.
  # CheckRun entries carry `status`/`conclusion`; StatusContext entries
  # carry `state`. SKIPPED and NEUTRAL count as passing.
  if [ "$checks_passed" = false ]; then
    counts=$(echo "$json" | jq -r '
      [.statusCheckRollup[]?] as $all
      | ($all | map(select(
          (.status == "COMPLETED" and (.conclusion | IN("SUCCESS","SKIPPED","NEUTRAL")))
          or .state == "SUCCESS"
        )) | length) as $passing
      | ($all | map(select(
          (.status == "COMPLETED" and (.conclusion | IN("FAILURE","TIMED_OUT","CANCELLED","ACTION_REQUIRED","STALE")))
          or (.state | IN("FAILURE","ERROR"))
        )) | length) as $failing
      | "\($all | length) \($passing) \($failing)"
    ' 2>/dev/null)
    read -r total passing failing <<<"$counts"
    if [ "${failing:-0}" -gt 0 ] 2>/dev/null; then
      echo "PR #PR_NUM has $failing failing check(s) — investigate before merging."
      checks_passed=true  # don't re-notify
    elif [ "${total:-0}" -gt 0 ] 2>/dev/null && [ "$total" = "$passing" ]; then
      echo "All $total checks passed on PR #PR_NUM — ready to merge!"
      checks_passed=true
    fi
  fi
  sleep 30
done
```

Tell the user to go review and merge the PR. While the monitor runs, you
can respond to other questions — the monitor notifications will arrive
asynchronously.

When the monitor reports `MERGED`, proceed to the next step.
If it reports `CLOSED`, inform the user and stop the release.

---

## Step 8 — Tag Release

Back in the working directory:

```bash
git checkout main
git pull
git tag "v<VERSION>"
git push --tags
```

Tell the user the tag was pushed and the release CI workflow has been
triggered.

---

## Step 9 — Publish to crates.io

**If this is a prerelease**, skip this step entirely and tell the user.

**If this is a stable release**, ask the user whether to publish.

If yes, publish each crate **in dependency order** using `--no-verify`
(the code already passed CI, and verification fails when crates.io
hasn't indexed a freshly-published dependency yet):

```
atuin-common, atuin-client, atuin-ai, atuin-dotfiles, atuin-history,
atuin-nucleo/matcher, atuin-nucleo, atuin-daemon, atuin-kv,
atuin-scripts, atuin-server-database, atuin-server-postgres,
atuin-server-sqlite, atuin-server, atuin-pty-proxy, atuin
```

For each crate, run from `crates/<name>`:
```bash
cargo publish --no-verify 2>&1
```

If it fails with "already uploaded", report it as a skip (not an error) —
some crates like `atuin-nucleo` are versioned independently and may
already be published at their current version.

If it fails for any other reason, stop and report the error.

---

## Completion

Summarize what was done:
- Version released
- PR URL
- Tag name
- Which crates were published (if any)
- Working directory path and how to clean it up (`rm -rf`)
