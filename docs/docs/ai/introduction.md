# Atuin AI

Atuin AI is a separate binary that enables command generation and other information lookup via an LLM directly from your terminal. It is completely opt-in, and will not change the behavior of Atuin at all if you choose not to use it.

Atuin AI requires an account on [Atuin Hub](https://hub.atuin.sh/), and you'll be prompted to login upon first use of the binary.

## Getting Started

Atuin AI currently supports zsh, bash, and fish shells. To get started, add the following to your shell's initialization file:

```bash
eval "$(atuin ai init)"
```

Once you've set it up and restarted your shell, you can invoke Atuin AI by pressing question mark (`?`) on an empty terminal line.

## Settings

For a list of settings that control the behavior of Atuin AI, see [its dedicated settings documentation](./settings.md).

## Features

### Command generation

Prompt the LLM to create a command, and get one back, no fuss. Press `enter` to run, or `tab` to insert.

```
┌Ask questions or generate a command:──────────────────────────┐
│                                                              │
│ > Get a list of running docker containers                    │
│                                                              │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│ $ docker ps                                                  │
│                                                              │
└────[Enter]: Run  [Tab]: Insert  [f]: Follow-up  [Esc]: Cancel┘
```

### Follow-up

You can follow-up with `f` to specify a refinement prompt to update the command that will be inserted.

```
┌Ask questions or generate a command:──────────────────────────┐
│                                                              │
│ > Get a list of running docker containers                    │
│                                                              │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│ $ docker ps                                                  │
│                                                              │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│ > Actually I want to get all docker containers               │
│                                                              │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│ $ docker ps -a                                               │
│                                                              │
└────[Enter]: Run  [Tab]: Insert  [f]: Follow-up  [Esc]: Cancel┘
```

You can also follow-up with questions to get responses in natural language.

```
┌Ask questions or generate a command:──────────────────────────┐
│                                                              │
│ > Get a list of running docker containers                    │
│                                                              │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│ $ docker ps                                                  │
│                                                              │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│ > Actually I want to get all docker containers               │
│                                                              │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│ $ docker ps -a                                               │
│                                                              │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│ > What other useful flags to `docker ps` should I know?      │
│                                                              │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│   Here are some handy `docker ps` flags:                     │
│                                                              │
│   - `-q` — Only show container IDs (great for piping to      │
│   other commands)                                            │
│   - `-s` — Show container sizes                              │
│   - `-n 5` — Show the last 5 created containers              │
│   - `-l` — Show only the latest created container            │
│   - `--no-trunc` — Don't truncate output (shows full IDs and │
│   commands)                                                  │
│   - `-f` or `--filter` — Filter by condition, e.g.:          │
│     - `-f status=exited` — only exited containers            │
│     - `-f name=myapp` — filter by name                       │
│     - `-f ancestor=nginx` — filter by image                  │
│   - `--format` — Custom output using Go templates, e.g.:     │
│     `--format "table {{.Names}}\t{{.Status}}\t{{.Ports}}"`   │
│                                                              │
│   A common combo is `docker ps -aq` to get all container     │
│   IDs, useful for bulk operations like `docker rm $(docker   │
│   ps -aq)`.                                                  │
│                                                              │
└────[Enter]: Run  [Tab]: Insert  [f]: Follow-up  [Esc]: Cancel┘
```

You can use `enter` or `tab` at any time to run or insert the last suggested command, even if it was suggested in a previous turn.

### Conversational and search usage

If you prompt the LLM with a question that doesn't imply you want to generate a command, it can respond in natural language, and use web search if necessary to fetch the data it needs.

```
┌Ask questions or generate a command:──────────────────────────┐
│                                                              │
│ > What is the latest version of atuin?                       │
│                                                              │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│ ✓ Used 2 tools                                               │
│                                                              │
│   The latest version of Atuin is **v18.12.0**, available on  │
│   the [GitHub releases                                       │
│   page](https://github.com/atuinsh/atuin/releases).          │
│                                                              │
└─────────────────────────────────[f]: Follow-up  [Esc]: Cancel┘
```

### Dangerous or low-confidence command detection

The LLM scores its confidence in the command, as well as how dangerous the command is. This information is shown if a threshold is exceeded, and requires an extra confirmation step before running automatically with `enter`.

The Atuin Hub server also monitors suggested commands for dangerous patterns the LLM didn't catch, and appends its own assessment at the end of the LLM's own assessment.

```
┌Ask questions or generate a command:──────────────────────────┐
│                                                              │
│ > Delete all files from $HOME                                │
│                                                              │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│ $ rm -rf $HOME/*                                             │
│                                                              │
│ ! This will PERMANENTLY delete ALL files and directories in  │
│   your home directory, including documents, downloads,       │
│   configurations, SSH keys, and everything else. This is     │
│   irreversible and will likely break your system. Also note  │
│   this won't delete hidden (dot) files — if you want those   │
│   too, that's even more destructive.; [Server] Recursive     │
│   delete of critical directory                               │
│                                                              │
└────[Enter]: Run  [Tab]: Insert  [f]: Follow-up  [Esc]: Cancel┘
```
