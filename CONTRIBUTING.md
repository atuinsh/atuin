# Contributing

Thank you so much for considering contributing to Atuin! We really appreciate it <3

## AI

While we are very happy for you to use AI to build your contribution, it is essential that any pull requests you open are reviewed, understood, and tested by you. Pull requests with giant, AI-written descriptions imply the opposite. Please take the time to write a PR body that outlines your intent, understanding and summary of the change.

## Development dependencies

1. A rust toolchain ([rustup](https://rustup.rs) recommended)

We commit to supporting the latest stable version of Rust - nothing more, nothing less, no nightly.

Before working on anything, we suggest taking a copy of your Atuin data directory (`~/.local/share/atuin` on most \*nix platforms). If anything goes wrong, you can always restore it!

While data directory backups are always a good idea, you can instruct Atuin to use custom path using the following environment variables:

```shell
export ATUIN_RECORD_STORE_PATH=/tmp/atuin_records.db # path to primary record store
export ATUIN_DB_PATH=/tmp/atuin_dev.db               # path to materialized history database
export ATUIN_KV__DB_PATH=/tmp/atuin_kv.db            # path to key-value store
export ATUIN_SCRIPTS__DB_PATH=/tmp/atuin_scripts.db  # path to scripts database
export ATUIN_AI__DB_PATH=/tmp/atuin_ai_sessions.db   # path to AI sessions database
export ATUIN_META__DB_PATH=/tmp/atuin_meta.db        # path to meta database
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

## Editor setup (optional)

`cargo +nightly fmt` is the source of truth and CI enforces it. A nightly version of Rust is required for formatting because Atuin's `.rustfmt.toml` uses nightly-only options.

- [`.editorconfig`](https://editorconfig.org/) which sets sane defaults for `.rs` files.
- [`.nvim.lua`](https://github.com/atuinsh/atuin/blob/main/.nvim.lua) which configures neovim. Note you need `vim.o.exrc = true` and `nvim >= 0.11+`. You can disable it if you prefer, please see the docs at the top of that file.

If you format via rust-analyzer (LSP integration in your editor), make sure it uses nightly Rustfmt by setting [`rust-analyzer.rustfmt.overrideCommand`](https://rust-analyzer.github.io/book/configuration.html#rustfmt.overrideCommand) to `["rustfmt", "+nightly"]`.

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

## Documentation

Docs live in `docs/docs/` and are built with mkdocs. To preview them:

```shell
cd docs && uv run mkdocs serve
```

Prose is linted with [Vale](https://vale.sh) against the Microsoft, proselint,
alex, and write-good style guides. CI fails on errors; warnings and suggestions
aren't annotated on the PR, but are visible when you run Vale locally.

Vale is not vendored — install it with your package manager
([docs](https://vale.sh/docs/install)), then fetch the pinned style packages
once:

```shell
brew install vale   # or your platform's equivalent
vale sync
```

Then, from the repo root:

```shell
vale docs/docs                              # everything, all severities
vale --minAlertLevel=error docs/docs        # only what CI gates on
vale docs/docs/guide/sync.md                # one file
```

Add `--no-global` if you keep a personal `~/.vale.ini`, so your local config
cannot change the result.

If Vale flags a technical term as a misspelling, first check whether it belongs
in backticks — config keys, flags, and command names usually do, which fixes
both the alert and the rendering. If it is genuinely prose, add it to
`.vale/styles/config/vocabularies/Atuin/accept.txt`, which is sorted
case-insensitively. Entries are regular expressions: `[Zz]sh` accepts both
casings, while a bare `Atuin` makes that casing canonical and flags every other
one.

Rule severities and the rules we have turned off live in `.vale.ini`, each with
a comment explaining why.

### One thing Vale cannot see

Vale parses Markdown as CommonMark, where any 4-space-indented block is a code
block. mkdocs admonition bodies are indented 4 spaces, so **Vale never lints
them**:

```markdown
!!! note

    This paragraph is invisible to Vale. Write it carefully.
```

Roughly 80 lines across 17 pages sit inside admonitions. They were cleaned by
hand once, but CI will not catch a regression there — so proof-read admonition
bodies yourself.

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

### Profiling

The team mostly profiles Atuin through either instrumented profiling or
statistical profiling. For a quick guide on profiling, a solid resource is
[Mathieu Ropert's "The Basics of Profiling"
talk](https://www.youtube.com/watch?v=dToaepIXW4s).

#### Instrumented Profiling

For our instrumented profiling, we use
[`tracing`](https://docs.rs/tracing/latest/tracing/) and the opentelemetry
crates. These export [OTel](https://opentelemetry.io/) spans into
[Jaeger](https://www.jaegertracing.io/).

To get started , you **must** build Atuin from source, with the
`profiling-traced` profile **and** the matching `profiling-traced` feature (which
compiles in the OpenTelemetry exporter). The `cargo build-traced` alias enables
both:

```bash
cargo build-traced
# equivalently:
# cargo build -p atuin --profile profiling-traced --features profiling-traced
```

> [!NOTE]
>
> The `profiling-traced` feature **must** be enabled.

> [!WARNING]
> 
> The `profiling-traced` profile is **necessarily** slower than the published
> `dist` profile.
>
> It builds on the `profiling` profile, described later, and on top of that
> enables `tracing`-level spans, which add latency to function entry and exit.
>
> You will not be lucky in profiling low-level algorithms, memory-alignment,
> etc. this way. If you are optimizing a tight loop, the statistical approach
> (described further down) will be of help.

With this, you can run Jaeger, like so, in one terminal:

```bash
docker run --rm --name jaeger \
  -p 16686:16686 \
  -p 4318:4318 \
  jaegertracing/jaeger:latest
```

You can then, in another terminal, run the `daemon`:

```bash
ATUIN_OTEL=http://localhost:4318 ./target/profiling-traced/atuin daemon start
```

Finally, you can run the client:

```bash
ATUIN_OTEL=http://localhost:4318 ./target/profiling-traced/atuin sync
```

> [!NOTE]
>
> You may need to tweak your exporting rate with the following env values:
>
> ```bash
> OTEL_BSP_MAX_QUEUE_SIZE=262144      # max span count in the queue
> OTEL_BSP_MAX_EXPORT_BATCH_SIZE=8192 # how many spans to push a time
> OTEL_BSP_SCHEDULE_DELAY=500         # how often to publish spans
> ```

You can navigate to `http://localhost:16686` in your local browser and you
should see your traces.

> [!NOTE]
>
> Using OTEL feels heavy here, but there is no simpler approach available
> off-the-shelf. The author wanted to use
> [`tracing-chrome`](https://docs.rs/tracing-chrome/latest/tracing_chrome/),
> but the crate does not gracefully handle `async` futures, so it wasn't
> useful.

#### Statistical Profiling

Statistical profiling should use `cargo build --profile profiling`. Unlike
`profiling-traced`, this will compile-away the `trace`-level events/spans,
which is more representative and closer to the `--profile dist` which is
published.

> You can profile the application however you like. Some of the team members
> use [`Instruments.app`](https://developer.apple.com/tutorials/instruments).
> 
> [!NOTE]
> If you are using `Instruments.app`, the easiest way is to use
> `cargo-instruments`:
> 
> ```bash
> cargo install cargo-instruments
> RUSTFLAGS="-C llvm-args=--inline-threshold=25" \
>   cargo instruments -t time --profile profiling -p atuin --bin atuin -- search "hello world"
> ```

## Migrations

Be careful creating database migrations - once your database has migrated ahead
of current stable, there is no going back.
