<!--
  Glossary source — auto-appended to every docs page by pymdownx.snippets (see
  mkdocs.yml). Each `*[Term]: definition` line turns matching prose into an
  <abbr> tooltip. Written for newcomers: never define a term with another term a
  beginner would also need looked up.

  GOTCHAS:
  - Keep every definition on ONE line. A wrapped line silently truncates the
    tooltip and renders the overflow as a stray code block on every page.
  - This file lives OUTSIDE docs/docs/ so Vale never lints it and MkDocs never
    builds it as a page. Keep it in sync with docs/docs/glossary.md.
-->

*[CLI]: Command-line interface — a program you use by typing commands in a terminal instead of clicking around a window
*[Docker]: A tool that bundles a program with everything it needs so it runs the same way on any computer; each bundle is called a container
*[E2E encryption]: End-to-end encryption — your data is scrambled on your own device before it leaves it, so nobody in between (not even the server) can read it
*[Hub]: Atuin Hub — Atuin's hosted service that stores your account and syncs your history for you
*[Kubernetes]: A system for running and scaling lots of containers across many machines; one way to self-host Atuin at larger scale
*[LLM]: Large language model — the kind of AI (like Claude or GPT) that powers Atuin's command generation
*[MCP]: Model Context Protocol — a shared standard that lets AI assistants plug into tools like Atuin, for example to search your history
*[PTY]: Pseudo-terminal — the behind-the-scenes channel a program uses to send text to and read text from your terminal
*[PostgreSQL]: A popular, powerful open-source database; the recommended storage for a self-hosted Atuin server
*[Postgres]: Short for PostgreSQL — a popular open-source database; the recommended storage for a self-hosted Atuin server
*[SQLite]: A lightweight database that lives in a single file on your computer; where Atuin keeps your shell history locally
*[SSH]: Secure Shell — a secure way to log in to and run commands on another computer over a network
*[TLS]: The standard encryption that protects data as it travels the internet — the same technology behind the padlock in your browser
*[UUID]: A long, randomly generated ID that is effectively unique, so different machines can create them without ever clashing
*[daemon]: A program that runs quietly in the background; Atuin's makes history writes instant and syncs on a schedule
*[dotfiles]: Your shell's personal settings — aliases, functions, and environment variables — which Atuin can sync across your machines
*[filter mode]: Which slice of your history a search looks through — everything, just this machine, this shell session, this directory, or this project
*[filter modes]: Which slice of your history a search looks through — everything, just this machine, this shell session, this directory, or this project
*[frecency]: A ranking that blends how often and how recently you have run a command, so your go-to commands show up first
*[fuzzy search]: Search that finds a command even when you type only a few of its letters, in order, typos and all
*[precmd]: A hook your shell runs right after each command; Atuin uses it to record how the command turned out
*[preexec]: A hook your shell runs right before each command; Atuin uses it to record the command as you run it
*[pty-proxy]: An Atuin feature that quietly watches a terminal session so it can capture the output of the commands you run
*[record store]: The secure, tamper-evident log at the core of Atuin's sync; your history, dotfiles, and other synced data are all kept here
*[regex]: Regular expression — a compact pattern for matching text, for example finding every command that starts with "git"
*[search mode]: How Atuin matches what you type against your history — fuzzy, full-text, prefix, or skim
*[search modes]: How Atuin matches what you type against your history — fuzzy, full-text, prefix, or skim
*[systemd]: The service manager on most Linux systems; it starts and supervises background programs like the Atuin daemon or server
*[workspace]: A filter mode that narrows search to the Git project you are currently working inside
