# Glossary

Definitions of the Atuin-specific and general terms used throughout these docs.
Terms defined here also show as tooltips when you hover them on any page.

## Core concepts

### Atuin

The tool these docs describe. Atuin replaces your shell's built-in history with a
SQLite database, records context for every command (working directory, exit code,
duration, and hostname), and optionally syncs that history across machines with
end-to-end encryption.

### TUI

The text user interface — Atuin's full-screen search screen. It opens when you
press <kbd>Ctrl-r</kbd> or the up arrow, and lets you search, filter, and run past
commands.

### Filter mode

The scope Atuin searches. The modes are global (all history), host (this machine),
session (the current shell session), directory (the current directory), and
workspace (the current Git project). You cycle through them with <kbd>Ctrl-r</kbd>.

### Search mode

How Atuin matches what you type against your history: fuzzy, full-text, prefix, or
skim. Fuzzy is the default.

### Fuzzy search

A search mode that matches your letters in order but tolerates gaps and typos
between them, so `gco` can find `git checkout`.

### Frecency

A ranking that blends how *frequently* and how *recently* a command was run, so the
commands you use most and most recently rise to the top of the results.

### Workspace

A filter mode that scopes results to the Git project you are currently inside,
following the directory tree up to the repository root.

### Dotfiles

Shell aliases, functions, and environment variables. Atuin can store and sync these
across your machines alongside your history.

### Hub

Atuin Hub — the hosted web service for accounts, runbooks, and team collaboration.

## Storage and sync

### Record store

Atuin's encrypted, append-only log. Every kind of synced data — history, dotfiles,
and more — is built on top of the record store.

### KV store

Atuin's key-value store, layered on top of the record store. It backs features such
as synced dotfiles.

### End-to-end encryption

Your data is encrypted on your own machine before it's uploaded. The sync server
stores only ciphertext and never sees your commands in plaintext. Also written E2E
encryption.

### Daemon

A background Atuin process that batches writes and syncs on a timer, so your shell
stays responsive.

### SQLite

The embedded, file-based database engine Atuin uses to store your local history.

## Shell integration

### Shell

The program that reads and runs the commands you type, such as bash, zsh, fish, or
Nushell. Atuin integrates with your shell through hooks.

### Preexec

The shell hook Atuin uses to record a command the moment before it runs.

### Precmd

The shell hook Atuin uses to capture a command's exit code and duration after it
finishes.

### PTY

A pseudo-terminal — the virtual terminal device that a program reads input from and
writes output to. Atuin uses one to capture command output.

### pty-proxy

An Atuin wrapper that sits on the PTY to capture the output of the commands you run,
which powers features such as reading command output for AI.

## AI

### MCP

The Model Context Protocol — a standard interface that exposes Atuin's history
search and command output to AI tools such as Claude Code and Cursor.

### LLM

A large language model — the AI that powers Atuin's command generation and lookup.

### Agent

An AI coding tool, such as Claude Code, Codex, or pi. Atuin can capture the commands
these agents run.

## General computing

### CLI

A command-line interface — a program you drive by typing commands, as opposed to a
graphical interface.

### SSH

Secure Shell — an encrypted protocol for logging into and running commands on remote
machines.

### TLS

Transport Layer Security — the encryption that protects HTTPS connections, including
sync traffic to an Atuin server.

### Regex

A regular expression — a compact pattern language for matching text, used in several
Atuin filters.

### UUID

A 128-bit identifier that's unique without any central coordinator. Atuin uses them
to identify records.

### systemd

The Linux service manager. You can run the Atuin daemon or a self-hosted server as a
systemd service.

### PostgreSQL

The production database backend for a self-hosted Atuin server. Often shortened to
Postgres.

### Docker

A tool that packages and runs software in isolated containers. Atuin ships an
official server image.

### Kubernetes

A system for running and scaling containers across a cluster, with a documented
self-hosting path.

### API

An application programming interface — the defined way one program talks to another,
such as the Atuin client talking to a sync server.
