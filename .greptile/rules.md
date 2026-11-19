# Review rules

`AGENTS.md` contains some instructions that help AI agents write better code and prose. These
instructions are not requirements for merging a pull request. Do not report violations of the
following instructions as findings, and do not leave review comments about them.

- The "Writing style" section of `AGENTS.md`. Humans are free to write comments, commit messages,
  and prose in their own style. The section is strict because agents tend to write prose that is
  hard to read without it.
- The instruction in the "Testing" section to migrate plain `#[test]`s to `rstest` in files that a
  change touches. A pull request that leaves an existing `#[test]` unchanged is fine.
