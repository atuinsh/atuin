# Contributing

Thank you so much for considering contributing to Atuin! We really appreciate it <3

Development dependencies

1. A rust toolchain ([rustup](https://rustup.rs) recommended)

We commit to supporting the latest stable version of Rust - nothing more, nothing less, no nightly.

Before working on anything, we suggest taking a copy of your Atuin data directory (`~/.local/share/atuin` on most \*nix platforms). If anything goes wrong, you can always restore it!

While data directory backups are always a good idea, you can instruct Atuin to use custom path using the following environment variables:

```shell
export ATUIN_RECORD_STORE_PATH=/tmp/atuin_records.db
export ATUIN_DB_PATH=/tmp/atuin_dev.db
export ATUIN_KV__DB_PATH=/tmp/atuin_kv.db
export ATUIN_SCRIPTS__DB_PATH=/tmp/atuin_scripts.db
```

It is also recommended to update your `$PATH` so that the pre-exec scripts would use the locally built version:

```shell
export PATH="./target/release:$PATH"
```

If you'd like to load a different configuration file, set `ATUIN_CONFIG_DIR` to a folder that contains your `config.toml` file:

```shell
export ATUIN_CONFIG_DIR=/tmp/atuin-config/
```

These variable exports can be added in a local `.envrc` file, read by [direnv](https://direnv.net/).

## PRs

It can speed up the review cycle if you consent to maintainers pushing to your branch. This will only be in the case of small fixes or adjustments, and not anything large. If you feel OK with this, please check the box on the template!

## What to work on?

Any issues labeled "bug" or "help wanted" would be fantastic, just drop a comment and feel free to ask for help!

If there's anything you want to work on that isn't already an issue, either open a feature request or get in touch on the [forum](https://forum.atuin.sh)/Discord.

## Setup

```
git clone https://github.com/atuinsh/atuin
cd atuin
cargo build
```

## Running

When iterating on a feature, it's useful to use `cargo run`

For example, if working on a search feature

```
cargo run -- search --a-new-flag
```

While iterating on the server, I find it helpful to run a new user on my system, with `sync_server` set to be `localhost`.

## Tests

Our test coverage is currently not the best, but we are working on it! Generally tests live in the file next to the functionality they are testing, and are executed just with `cargo test`.

## Logging and Debugging

### Log Files

Atuin writes logs to `~/.atuin/logs` unless configured otherwise. Log files are rotated daily and retained for 4 days by default:

- `search.log.*` - Interactive search session logs
- `daemon.log.*` - Background daemon logs

### Log Levels

You can set the `ATUIN_LOG` environment variable to override log verbosity from the config file:

```shell
ATUIN_LOG=debug atuin search  # Enable debug logging
ATUIN_LOG=trace atuin search  # Enable trace logging (very verbose)
```

### Span Timing (Performance Profiling)

For performance analysis, you can capture detailed span timing data as JSON:

```shell
ATUIN_SPAN=spans.json atuin search
```

This creates a JSON file with timing information for each instrumented span, including:
- `time.busy` - Time actively executing code
- `time.idle` - Time awaiting async operations (I/O, child tasks)

The `scripts/span-table.ts` script analyzes these logs:

```shell
# Summary view - shows all spans with timing stats
bun scripts/span-table.ts spans.json

# Detail view - shows individual calls for a specific span
bun scripts/span-table.ts spans.json --detail daemon_search

# Filter to specific spans
bun scripts/span-table.ts spans.json --filter "search|hydrate"
```

This is useful for comparing performance between different search implementations or identifying bottlenecks.

## Migrations

Be careful creating database migrations - once your database has migrated ahead of current stable, there is no going back

### Stickers

We try to ship anyone contributing to Atuin a sticker! Only contributors get a shiny one. Fill out [this form](https://noteforms.com/forms/contributors-stickers) if you'd like one.
