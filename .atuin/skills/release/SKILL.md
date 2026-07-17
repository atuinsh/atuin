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

Verify these tools are installed: `git`, `cargo`, `gh`, `git-cliff`.

On macOS, also verify `gsed`. On Linux, `sed` is already GNU sed.

Use `command -v` for each. If any are missing, report which ones and stop.

Set the `SED` variable for later use: if on macOS, `SED=gsed`; on Linux, `SED=sed`.

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

## Step 2.5 — Survey Release-Labeled PRs

Two labels mark PRs whose merge has to be timed around the release. They are
typically PRs touching `docs/` or `install.sh` — changes that deploy on merge to
`main` rather than shipping inside the release artifact.

| Label | Merged at | In the tag & artifact? |
|-------|-----------|------------------------|
| `merge-immediately-before-release` | Step 7.5 — after the prep PR merges, before the tag | Yes |
| `merge-immediately-after-release` | Step 8.5 — after the release workflow uploads assets | No |

Check both:

```bash
gh pr list --repo atuinsh/atuin --state open \
    --label merge-immediately-before-release \
    --json number,title,url
gh pr list --repo atuinsh/atuin --state open \
    --label merge-immediately-after-release \
    --json number,title,url
```

If both come back empty, say so and continue to Step 3. No checkpoint needed.

The two labels are mutually exclusive: they name opposite sides of the tag. If a
PR number comes back in **both** lists, that is a labelling mistake — the two
queries are independent, so it would otherwise be approved twice, merged at Step
7.5, then merged again at Step 8.5 against an already-closed PR. **Stop** and ask
the user which single timing applies, then treat it as carrying only that label
for the rest of the release. Do not guess, and do not carry it into both steps.

If either is non-empty:

1. Show the user every PR found, grouped by label, with its number, title, and
   the merge point from the table above.
2. **Checkpoint:** Use AskUserQuestion (multiSelect) to ask which of these PRs
   should in fact be merged at their labeled time.
3. Record the approved set — it carries to Step 7.5 and Step 8.5. A PR the user
   declines is left alone for the remainder of the release: don't merge it,
   don't raise it again.

Take each label at face value. It records the PR author's decision about timing;
it is not a suggestion for you to re-evaluate:

- An `after-release` PR is **not** in the tag and **not** in the release
  artifact. That is the intended outcome, not a problem to solve. The label is
  for changes that deploy the moment they merge, so artifact inclusion is
  irrelevant to them. An author who needed the change in the artifact would have
  labeled it `merge-immediately-before-release`.
- Never merge an `after-release` PR early to get it into the release, and never
  reclassify a PR from one label to the other.

---

## Step 3 — Set Up Working Directory

Clone a fresh copy into a temp directory:

```bash
WORKDIR=$(mktemp -d)
git clone git@github.com:atuinsh/atuin.git "$WORKDIR"
```

Print the working directory path so the user can find it if needed.

NOTE:
ALL subsequent Bash commands run from `$WORKDIR`.

---

## Step 4 — Create Branch & Update Versions

1. Create a release branch named after the version (no `v` prefix):
   `git checkout -b <VERSION>`

2. Replace the old version with the new one in all `Cargo.toml` files.
   **Escape dots** in the old version so sed treats them literally:

   ```bash
   VERSION_PATTERN="${OLD_VERSION//./\\.}"
   find . -type f -name 'Cargo.toml' -not -path './.git/*' \
       -exec $SED -i "s/$VERSION_PATTERN/$NEW_VERSION/g" {} \;
   ```

3. Run `cargo check` to update `Cargo.lock`.

4. Show `git diff --stat` and the version-related lines from the diff:
   ```bash
   git diff --unified=0 -- '*.toml' | grep '^\+.*version' | grep -vF '+++'
   ```
   Remember to use macOS grep arguments on macOS systems.

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
    --repo atuinsh/atuin \
    --draft
```

Show the PR URL to the user. Tell the user to go review and merge the PR.

When the user reports the PR is merged, proceed to the next step.

---

## Step 7.5 — Merge `merge-immediately-before-release` PRs

Skip to Step 8 unless Step 2.5 recorded approved PRs with this label.

The prep PR has merged and the tag does not exist yet. This is the only window
in which these PRs land in `v<VERSION>` and its artifacts.

For each approved PR:

1. Check it is still mergeable and its checks are green:
   ```bash
   gh pr view <N> --repo atuinsh/atuin \
       --json mergeable,mergeStateStatus,statusCheckRollup
   ```
   If it is not mergeable, or checks are red, report that and ask the user how
   to proceed. Do not force it through.

2. **Checkpoint:** Show the PR number and title and ask the user to confirm this
   specific merge. Step 2.5 approved the *timing*; this confirms merging *now*.

3. Merge:
   ```bash
   gh pr merge <N> --repo atuinsh/atuin --squash
   ```

These commits land in the tag but are absent from the changelog generated in
Step 5, which was cut before they merged. That is expected — do not regenerate
the changelog to include them.

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

## Step 8.5 — Merge `merge-immediately-after-release` PRs

Skip to Step 9 unless Step 2.5 recorded approved PRs with this label.

"After the release" means **after the release workflow has finished and the
assets exist** — not merely after the tag was pushed. These PRs deploy the
instant they merge, so a docs change announcing the new version must not go
live while the release it points at is still building.

1. Wait for the release workflow the tag just triggered:
   ```bash
   gh run list --repo atuinsh/atuin --workflow release.yml \
       --branch "v<VERSION>" --limit 1
   gh run watch <run-id> --repo atuinsh/atuin --exit-status
   ```
   `--exit-status` is what makes a failed run exit non-zero. Without it
   `gh run watch` prints the failure but still exits `0`, and a broken release
   sails through into the merges below. If it exits non-zero, **stop** here —
   do not go on to the release lookup.

2. Confirm the release exists and has assets attached:
   ```bash
   gh release view "v<VERSION>" --repo atuinsh/atuin --json assets,isDraft
   ```
   If the release is missing, is still a draft, or has no assets, **stop**.
   Report it and merge nothing — these PRs keep until the release is actually
   out. A release object alone is not evidence: it may predate this run or have
   been left half-built by a failed workflow. What clears this gate is a
   non-draft release with assets attached.

3. Re-check that each approved PR is still mergeable and its checks are green:
   ```bash
   gh pr view <N> --repo atuinsh/atuin \
       --json mergeable,mergeStateStatus,statusCheckRollup
   ```
   Step 2.5 surveyed these PRs before the release was even built, so its
   findings are now many minutes stale — long enough for a PR to pick up a
   conflict or a red check. If it is not mergeable, or checks are red, report
   that and ask the user how to proceed. Do not force it through. These PRs
   deploy the instant they merge, so there is no CI gate downstream to catch a
   bad one.

4. **Checkpoint:** For each approved PR, show its number and title and ask the
   user to confirm this specific merge. Step 2.5 approved the *timing*; this
   confirms merging *now*.

5. Merge each PR the user confirms:
   ```bash
   gh pr merge <N> --repo atuinsh/atuin --squash
   ```

Merging a `docs/` change here fires `trigger-docs-deploy.yml` and the docs go
out immediately. That is the whole point of the label.

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
- Which label-timed PRs were merged, and which the user declined (if any)
- Which crates were published (if any)
- Working directory path and how to clean it up (`rm -rf`)
