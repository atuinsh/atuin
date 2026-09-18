# Harness session fixtures

These `.jsonl` files are **redacted structural skeletons** of real AI-harness
sessions (Claude Code, Codex, Pi), used to exercise the session normalizers in
`harnesstools` against the record shapes real harnesses actually emit — the
variety (thinking blocks, tool sequences, non-turn record types) that hand-written
synthetic fixtures miss.

They are **not** real session content. Every session was passed through a
default-deny redactor that:

- keeps only the JSON **structure** and the **dispatch discriminants** the
  normalizer reads (`type`, `role`, content-block `type`, and standard tool
  names such as `Bash`/`Read`/`Write`);
- replaces every other string value with `<redacted>`;
- rewrites filesystem paths to `<path>`;
- remaps every real id (session/message/tool-call uuid) to a deterministic
  synthetic uuid, preserving `tool_use` ↔ `tool_result` linkage within a session;
- normalizes every timestamp to a fixed value;
- replaces non-standard tool names with `tool`.

No prose, paths, credentials, model names, user identity, or timestamps from the
original sessions survive. Regenerate by re-running the redactor over local
session directories; do not hand-edit real content into these files.
