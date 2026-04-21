# Atuin - Project Summary

Atuin is an open-source tool that replaces your existing shell history with a SQLite database, recording additional context (exit code, duration, working directory, hostname, session, etc.) for every command you run. It is written in Rust and organized as a multi-crate workspace (v18.15.2).

## Key Features

- **Full-screen history search UI** bound to ctrl-r / up arrow
- **Encrypted sync** of shell history across machines via an Atuin server (self-hostable or hosted)
- **Cross-terminal, cross-session, cross-machine** unified history
- **Rich search** — filter by exit code, directory, time range, and more
- **Statistics** — most used commands, durations, and other insights
- **AI assistant** — built-in shell AI for command generation and task automation
- **Dotfiles & scripts management** via dedicated crates

## Supported Shells

zsh, bash, fish, nushell, xonsh, and powershell (tier 2).

## Architecture

The project is a Rust workspace with 15 crates including: `atuin` (CLI binary), `atuin-client`, `atuin-server`, `atuin-common`, `atuin-daemon`, `atuin-history`, `atuin-dotfiles`, `atuin-scripts`, `atuin-kv`, `atuin-ai`, `atuin-nucleo` (fuzzy matching), and server database backends for PostgreSQL and SQLite.

## License

MIT
