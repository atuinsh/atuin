<!--
  Glossary source — auto-appended to every docs page by pymdownx.snippets
  (see mkdocs.yml). Each `*[Term]: definition` line turns matching prose text
  into an <abbr> tooltip. Matches are case-sensitive and skip code spans.

  This file lives OUTSIDE docs/docs/ so Vale never lints its *[...] syntax and
  MkDocs never builds it as a page. Keep it in sync with docs/docs/glossary.md.
-->

<!-- Core concepts -->
*[TUI]: Atuin's full-screen terminal search interface
*[filter mode]: The scope of a search — global, host, session, directory, or workspace
*[filter modes]: The scope of a search — global, host, session, directory, or workspace
*[search mode]: How queries are matched — fuzzy, full-text, prefix, or skim
*[search modes]: How queries are matched — fuzzy, full-text, prefix, or skim
*[fuzzy search]: Matching that tolerates gaps and typos between the letters you type
*[frecency]: Ranking that blends how often and how recently a command was run
*[workspace]: A filter mode scoped to the current Git project directory tree
*[dotfiles]: Shell aliases, functions, and environment variables Atuin can sync across machines
*[Hub]: Atuin Hub — the hosted service for accounts, runbooks, and collaboration

<!-- Storage & sync -->
*[record store]: Atuin's encrypted, append-only log that all synced data is built on
*[KV store]: Atuin's key-value store, layered on top of the record store
*[end-to-end encryption]: Data is encrypted on your machine before it syncs; the server never sees plaintext
*[E2E encryption]: End-to-end encryption — data is encrypted on your machine before it syncs
*[daemon]: A background Atuin process that batches writes and syncs on a timer
*[SQLite]: The embedded database engine Atuin stores your history in

<!-- Shell integration -->
<!-- "shell" is intentionally NOT tooltipped: it recurs ~20x on some pages and
     the underline noise outweighs the value. It is defined on glossary.md. -->
*[preexec]: The shell hook Atuin uses to record a command just before it runs
*[precmd]: The shell hook Atuin uses to capture a command's result after it finishes
*[PTY]: Pseudo-terminal — the virtual terminal device a program reads from and writes to
*[pty-proxy]: An Atuin wrapper that sits on the PTY to capture command output

<!-- AI -->
*[MCP]: Model Context Protocol — a standard interface that exposes Atuin to AI tools
*[LLM]: Large Language Model — the AI that powers Atuin's command generation
*[agent]: An AI coding tool such as Claude Code, Codex, or pi

<!-- General computing -->
*[CLI]: Command-line interface — a tool you drive by typing commands
*[SSH]: Secure Shell — an encrypted protocol for logging into remote machines
*[TLS]: Transport Layer Security — the encryption behind HTTPS connections
*[regex]: Regular expression — a pattern language for matching text
*[UUID]: A 128-bit identifier that is unique without any central coordinator
*[systemd]: The Linux service manager used to run the Atuin daemon or server
*[PostgreSQL]: The production database backend for a self-hosted Atuin server
*[Postgres]: PostgreSQL — the production database backend for a self-hosted Atuin server
*[Docker]: A tool that packages and runs software in isolated containers
*[Kubernetes]: A system for running and scaling containers across a cluster
*[API]: Application programming interface — how programs talk to each other
