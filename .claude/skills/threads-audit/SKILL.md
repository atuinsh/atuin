---
name: threads-audit
description: Audit whether the changes in this repository match what was asked of the coding agents, by comparing the intent in captured Atuin threads, across every agent, with the git changes of the same window. Use before a release, or when asked whether recent changes do what was intended.
---

# Threads audit

Atuin captures each coding agent session as a thread. The prompts in the
threads say what was intended. Git says what changed. Audit the two against
each other, as a whole: the question is whether the repository now does what
was asked, not whether each commit has a thread. Only read: change no files,
and make no commits.

## 1. Set the window

This needs `atuin`, `git` and `jq`. If one is missing, say which and stop.

The window starts at a base commit. By default, the base is the last commit
that is at least 14 days old:

```sh
base=$(git rev-list -1 --before='14 days ago' HEAD)
```

When the user asks for "since last release", the base is the last tag:

```sh
base=$(git describe --tags --abbrev=0)
```

The user may also name a commit or a tag as the base, and a path to limit the
audit to. Then take the time of the base, in UTC as thread times are:

```sh
since=$(TZ=UTC0 git log -1 --date=format-local:'%Y-%m-%dT%H:%M:%S' --format=%cd "$base")
```

## 2. List the threads

A thread is in the audit when it ran in any worktree of the project and was
active after the base commit.

```sh
roots=$(git worktree list --porcelain | sed -n 's/^worktree //p' | jq -R . | jq -sc .)
atuin threads list --style ndjson | jq -c --argjson roots "$roots" --arg since "$since" '
  select(.updated_at >= $since)
  | select(.cwd as $c | $c != null and any($roots[]; . == $c or ($c | startswith(. + "/"))))
  | {id: .atuin_id, harness, branch: .git_branch, title: (.title // .preview[0:80])}'
```

If there are no threads, say that capture may be off, name
`atuin threads enable` and `atuin threads import`, and stop.

## 3. Collect the intents

Read the prompts of each thread that came after the base commit.

```sh
atuin threads show <id> --style json | jq -r --arg since "$since" '
  .messages[] | select(.role == "user" and .timestamp >= $since)
  | .timestamp + "  " + ([.content[] | select(.type == "text") | .text] | join(" ") | .[0:600])'
```

An intent is a change the user asked for. Questions, and requests that only
read, are not intents. Merge the prompts that refine one request into one
intent. When a later prompt reverses an earlier one, in any thread, the later
one stands. Keep the thread and its branch with each intent.

## 4. Read the changes

```sh
git log --format='%h %an %s' "$base"..HEAD
git diff --stat "$base"..HEAD
git diff "$base"..HEAD -- <path>
```

Read the net diff, not each commit: work that was redone inside the window
counts as it ended up.

## 5. Judge

Give each intent one verdict, and name the files that show it:

- **met**: the changes do what was asked.
- **partly met**: say what is missing.
- **not met**: nothing in the changes does it. When the thread ran on a branch
  that isn't merged into `HEAD`, say so: the work may be waiting there.
- **contradicted**: the changes go against it, or two intents that both stand
  can't both hold.

Then look the other way. List the changes that no intent accounts for. A
commit by another author is expected to have none, since these are only the
threads of the person running the audit. A change in the user's own commits
that nobody asked for is a finding.

## 6. Report

Start with one line: `threads-audit: pass` when every intent is met and the
user's own commits are all accounted for, and `threads-audit: attention`
otherwise. Then give the coverage: the threads and intents read, and how many
of the commits in the window are the user's own.

List only what needs attention, worst first. Cite threads by the last 8 digits
of the id with the agent, and commits by short hash. End with the count of
intents that were met.
