# Logging

Atuin keeps its logs in one file a day, `~/.atuin/logs/atuin.<date>.log`, or
`$ATUIN_HOME/logs` if you set `ATUIN_HOME`. The daemon and every `atuin`
command append to the same file. Atuin deletes a file 4 days after its last write.
The date in the name is in UTC, so a new file starts at midnight UTC.

## Reading the log

Each line is one event in [logfmt](https://brandur.org/logfmt), `key=value`
pairs that start with the process that wrote it:

```text
process=daemon pid=4127 ts=2026-10-20T14:03:11.520Z level=info target=atuin_daemon::daemon message="settings applied"
```

`process` is `daemon` for the daemon and `cli` for every other command. `pid`
tells two commands apart.

`grep` is enough to follow one of them:

```shell
grep process=daemon ~/.atuin/logs/atuin.2026-10-20.log
```

Log viewers that read logfmt, such as [hl](https://github.com/pamburus/hl) and
[lnav](https://lnav.org), can filter and colour it. This shows the daemon's
warnings and errors from every file:

```shell
hl -f process=daemon -l w ~/.atuin/logs/atuin.*.log
```

## Changing what Atuin logs

The `ATUIN_LOG` environment variable sets what the file records. It takes a
comma-separated list of levels (`error`, `warn`, `info`, `debug`, `trace`, or
`off`), each optionally for one module. Without it, Atuin records its own
events at `info` or higher and everything else at `warn` or higher.

```shell
# Everything at debug
ATUIN_LOG=debug atuin sync

# The daemon's internals at debug, the rest at warn
ATUIN_LOG=warn,atuin_daemon=debug atuin daemon start

# No log file at all
ATUIN_LOG=off atuin search
```

`ATUIN_LOG` only changes the file. Atuin skips a part of it that it can't read,
and says so. Release builds don't include `trace`.

## What appears in the terminal

Warnings and errors also print in the terminal, but never on top of the search
UI.

To watch every event the daemon logs as it happens, run it in the foreground:

```shell
ATUIN_LOG=debug atuin daemon start --show-logs
```

## Logging from a daemon you didn't start

A daemon that `atuin` starts for you has the environment of the command that
started it. Set `ATUIN_LOG` on a restart to change it:

```shell
ATUIN_LOG=debug atuin daemon restart
```

A daemon run by systemd reads its environment from the unit. Add
`Environment=ATUIN_LOG=debug` to the unit, or set it for your user's service
manager, then restart the unit:

```shell
systemctl --user set-environment ATUIN_LOG=debug
```
