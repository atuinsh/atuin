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

The audit surveys every thread and every commit of the window. It doesn't
single out one author or one machine.

The commands below are examples. Each one is a single `git` command, or
`atuin threads` piped into a `jq` filter. Values in `<angle brackets>` come
from the output of an earlier command: read them there and write them in.
Adapt the quoting to the shell. The data shapes are given so that a filter
can be rewritten when an example doesn't fit.

## 1. Set the window

This needs `atuin`, `git` and `jq`. If one is missing, say which and stop.

The window starts at a base commit. By default, the base is the last commit
that is at least 14 days old:

```sh
git rev-list -1 --before="14 days ago" HEAD
```

When the user asks for "since last release", the base is the last tag:

```sh
git describe --tags --abbrev=0
```

The user may also name a commit or a tag as the base, and a path to limit the
audit to. Then take the time of the base, in seconds since the epoch:

```sh
git log -1 --format=%ct <base>
```

## 2. List the threads

`atuin threads list --style ndjson` prints one object per thread:

| Field | Meaning |
| --- | --- |
| `atuin_id` | The id that `atuin threads show` takes. |
| `harness` | The agent, such as `claude-code`, `codex` or `opencode`. |
| `host` | The machine the thread ran on. |
| `cwd` | The directory the thread ran in. It can be missing. |
| `git_branch` | The branch at the time. It can be missing. |
| `started_at`, `updated_at` | UTC times, as `2026-11-10T06:48:56.793+00:00`. |
| `title`, `preview` | A title when there is one, and the start of the first prompt. |
| `parent_atuin_id` | Set on a subagent or a fork. It names the thread it came from. |

A thread is in the audit when it ran in any worktree of the project and was
active after the base commit. `git worktree list --porcelain` names the
worktrees, on the lines that start with `worktree`. Pass each path after
`--args`:

```sh
atuin threads list --style ndjson | jq -c --argjson base <base time> '
  ($base | todate | .[0:19]) as $since
  | select(.updated_at >= $since)
  | select(.cwd as $c | $c != null and any($ARGS.positional[]; . as $root | $root == $c or ($c | startswith($root + "/"))))
  | {id: .atuin_id, harness, host, branch: .git_branch, parent: .parent_atuin_id, title: (.title // .preview[0:80])}
' --args <worktree path> <worktree path>
```

The filter assumes `/` between the parts of a path. The prompts of a subagent
were written by its parent agent, not by a person, so read a thread with a
`parent` only for the files it wrote.

If there are no threads, say that capture may be off, name
`atuin threads enable` and `atuin threads import`, and stop.

## 3. Collect the intents

`atuin threads show <id> --style json` prints one object with two keys.
`session` holds the fields of the list. `messages` is an array, in order:

| Field | Meaning |
| --- | --- |
| `role` | `user` for a prompt and `assistant` for a reply. Other roles are bookkeeping of the agent. |
| `timestamp` | A UTC time, in the format of the list. |
| `content` | An array of parts. Each part has a `type`. |

The parts of a message, by `type`:

| Type | Fields |
| --- | --- |
| `text` | `text`: what was written. |
| `tool_call` | `name`, and `input` as a string of JSON. |
| `patch` | `files`: an array of `{path, change, additions, deletions, diff}`. |
| `thinking`, `tool_result`, `error` | Not needed for the audit. |

Read the prompts of each thread that came after the base commit:

```sh
atuin threads show <id> --style json | jq -r --argjson base <base time> '
  ($base | todate | .[0:19]) as $since
  | .messages[] | select(.role == "user" and .timestamp >= $since)
  | .timestamp + "  " + ([.content[] | select(.type == "text") | .text] | join(" "))'
```

An intent is a change the user asked for. Questions, and requests that only
read, are not intents. Merge the prompts that refine one request into one
intent. When a later prompt reverses an earlier one, in any thread, the later
one stands. Keep the thread and its branch with each intent.

The files a thread wrote show where its work went:

```sh
atuin threads show <id> --style json | jq -r '
  [.messages[] | .content[] | select(.type == "patch") | .files[].path] | unique | .[]'
```

Not every agent records a `patch`, and not every write becomes one. A path in
the list is evidence. A path that is missing from the list proves nothing.

## 4. Read the changes

```sh
git log --format="%h %an %s" <base>..HEAD
git diff --stat <base>..HEAD
git diff <base>..HEAD -- <path>
```

Read the net diff, not each commit: work that was redone inside the window
counts as it ended up.

Work can also be waiting outside the window. Run the first command in each
worktree:

```sh
git -C <worktree path> status --short
git branch --no-merged HEAD
```

## 5. Judge

Give each intent one verdict, and name the files that show it:

- **met**: the changes do what was asked.
- **partly met**: say what is missing.
- **not met**: nothing in the changes does it. Say where the work may be
  instead: uncommitted in a worktree, on a branch that isn't merged into
  `HEAD`, or in files outside the repository.
- **contradicted**: the changes go against it, or two intents that both stand
  can't both hold.

Then look the other way. Go through the changes of every commit in the
window, whoever the author is, and list the changes that no intent accounts
for. Each one is a finding. Threads exist only for the sessions that this
Atuin store holds, so say when a change has no thread from any host at
around its time: the request may have been made where no thread was kept.

## 6. Report

Start with one line: `threads-audit: pass` when every intent is met and every
change is accounted for, and `threads-audit: attention` otherwise. Then give
the coverage: the threads read, with their agents and hosts, the intents
found, and the commits in the window, with their authors.

List only what needs attention, worst first. Cite threads by the last 8 digits
of the id with the agent, and commits by short hash. End with the count of
intents that were met, and the count of commits that are accounted for.
