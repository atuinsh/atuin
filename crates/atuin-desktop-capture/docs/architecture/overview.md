# Desktop capture: overview

What desktop capture is for, the rules that it must keep, and how it runs
inside Atuin: the helper, the daemon component, and the store. [The
index](README.md) lists the other architecture documents.

## Summary

Much relevant context lives outside the terminal: CI pages, documentation,
issue trackers, chat, and editors. Accessibility APIs expose that context as
structured text, which is close to what Atuin already handles.

- **It works on both platforms.** AT-SPI on Linux and the AX API on macOS
  expose readable text and structure for browsers, editors, and office
  applications.
- **Cost is manageable.** Reading the visible content of a window takes 5 to 20
  ms for typical applications and 110 to 200 ms for a 3,000-line page in a
  browser.
- **Setup and privacy are the hard parts.** On Linux, Chromium and Electron
  join the accessibility bus only when a client is connected at their start or
  a session variable is set. macOS needs a separately signed helper. Secrets
  leak through URLs, titles, and panels even when password fields are masked.

## Invariants

The feature fails, even if it works, when any of these happen:

- Capture runs without an explicit opt-in, or reads applications that the user
  did not choose.
- Secrets reach storage or the model. Snapshot text goes through the same
  secret filtering as history, before it leaves the helper.
- A prompt waits on the desktop. The hook answers from what the daemon has
  already stored.
- The daemon hangs or crashes because an application misbehaves.
- The hot paths `history start`, `history end`, and `init` gain any work.

## In Atuin

When desktop capture is enabled, the daemon runs
`atuin-desktop-capture capture` and stores the records that the helper sends.
The records go to `<data_dir>/desktop-capture/`, and a full-text index of their
text goes to `<data_dir>/desktop-capture-index.sqlite`. Nothing is synced. The
store stays under `desktop_capture.max_disk_usage`, which is 10% of the disk by
default. Past the limit, the daemon deletes the oldest segments first. The
daemon restarts a helper that crashes, and it replaces a helper that does not
answer a ping within 30 seconds. It refuses a helper of another build, and
starts a helper again once the helper's file changes. After another failure,
the daemon tries again with a backoff that grows to a minute, and at once when
the helper's file changes. A reinstalled helper therefore starts without a
daemon restart.

The daemon looks for the helper next to its own executable. The `helper_path`
setting points the daemon at another helper.

A dialog is stored as a segment without a length. A background window gets a
segment of its own, which holds the window's reads until the window's
application, title, or URL changes, for an hour at most, and until capture
stops. Each read names its trigger: `swept` for the sweep at the start, and
otherwise the kind of change that led to the read.

## Processes

Desktop capture runs in Atuin as a helper process and a daemon component. Every
desktop read happens in the helper, so the daemon and the `atuin` CLI never
link accessibility code.

- **The helper, `atuin-desktop-capture`.** The `atuin` package builds it from
  the library as a second binary, with the `desktop-capture` feature. It is a
  dedicated binary, because on macOS the user grants the Accessibility
  permission to the binary that asks for it. The grant then covers only the
  code that reads the desktop. A separate process also keeps a hung or
  misbehaving application away from the daemon. The helper has four commands.
  The daemon runs `capture`. `atuin desktop-capture` runs `apps` and `doctor`
  with the terminal of the command, and `atuin setup` runs `access` to check
  for the Accessibility permission and to ask for it. The helper takes its
  build string from the daemon's crate, so the two binaries of one build always
  match.
- **The `desktop-capture` feature.** A build without the feature doesn't
  contain any of desktop capture. In `atuin-daemon`, the feature turns on the
  dependency on `atuin-desktop-capture`, the `desktop_capture` module, the gRPC
  service and its client, and the dependencies that only capture uses. The
  feature of `atuin` turns on the daemon's feature, the second binary, the
  subcommand, the Desktop row of `atuin setup`, the `desktop_capture` section
  of `atuin doctor`, and the prompt context of the Claude Code hook. `atuin-ai`
  registers the desktop tools only with the feature. The build string ends in
  `.desktop-capture` in a build with the feature, so a client replaces a daemon
  of the same commit that lacks the `DesktopCapture` service.
  `atuin-desktop-capture` has the shape of a plugin's name, so a build without
  the feature refuses `atuin desktop-capture`, rather than running a helper
  that an earlier build left on the `PATH` as a plugin.
  `atuin desktop-capture login` still succeeds quietly there, because a session
  that an earlier build set up runs it at each login. Only a build with the
  feature removes what setup wrote, so a user turns capture off before
  switching to a build without it. [Performance](performance.md) gives the cost
  of the feature to a build.
- **The link.** The helper's stdin and stdout carry length-prefixed MessagePack
  frames. The library's `link` module defines the messages, and
  `atuin_common::rmp::frame` writes and reads the frames. The helper's first
  frame names its build, and the daemon stops a helper of another build. The
  daemon sends the application selection, the exclusions, and the secret
  filter, together with whether the helper runs or stays paused. A helper that
  starts when capture is stopped therefore doesn't read anything. Later, the
  daemon pauses, resumes, and shuts down the helper. When capture is paused,
  the helper discards the events that the backend still delivers. For example,
  the Linux backend's monitor of the session bus keeps receiving notifications,
  and the helper drops them. The helper sends every record that `Capture`
  yields, after it filters secrets and scrubs one-time codes. Its stderr is its
  log, which the daemon forwards. Each side skips a frame whose body does not
  decode, with a warning, because the length prefix says where the next frame
  starts. A stream that fails, ends inside a frame, or announces a frame over
  64 MiB ends the link.
- **The daemon component, `desktop_capture`.** It sits beside `output_capture`
  and `session_capture`, and like them it is a plain struct that
  `atuin_daemon::boot` builds. Its supervisor is one task, which opens the
  store and runs the helper when capture is enabled. The task opens the store
  after the daemon starts to serve, so that the recovery of a large store
  doesn't delay the shell hooks or the other clients. A request that reads the
  store during the open waits up to 30 seconds for the open, so that the first
  `log` or `search`, which starts the daemon, gets an answer. When the store
  stays closed, because capture is off or the open failed, those requests fail
  as unavailable at once. A stop, a start, and a shutdown take effect during
  the open, and a deletion waits until the store is open. A store that fails to
  open shows as `Failed`, and the supervisor tries again with a backoff. A
  panic while the store opens also shows as `Failed`. The supervisor spawns the
  helper, which it finds next to the daemon's executable or at `helper_path`,
  and restarts it with a backoff when it exits. The backoff starts at a second
  and doubles up to a minute. During the backoff, the supervisor checks the
  helper's file every 2 s, and a new file ends the backoff at once, because a
  reinstall can fix a missing helper, a helper of another build, or a broken
  signature. A helper of another build isn't started again on a timer: the next
  helper starts once the helper's file changes. On macOS, the supervisor first
  signs a copy of the helper, and gives up on a signing that takes longer than
  2 minutes. The supervisor checks the shipped helper there, because a new
  shipped helper gets a new signed copy. A helper that lacks the permission is
  checked again at a long interval.

  A helper that doesn't find a desktop session, or whose session ends, waits
  for one, and the backoff starts over. How it waits depends on what the
  platform's watch of sessions knows. On macOS, a helper finds a desktop
  exactly when its user is at the screen, which the watch can check. The next
  helper then starts once the user is at the screen, at once when the user is
  there already. When the user is away, no helper starts, even when the daemon
  starts or the settings change. Only a `start` runs a helper then. A helper
  that doesn't find a desktop when the user is at the screen starts again with
  the backoff. On Linux, only a helper can find out whether a session lets it
  in, so the supervisor retries. It starts a helper again every 2 s for 2
  minutes. After those quick retries, the supervisor starts a helper only on a
  hint that a desktop session may have started, so that a machine that never
  has a desktop session, such as a server that shares the user's configuration,
  doesn't start helpers in a loop. Each hint starts a helper at once and brings
  back the quick retries, because a session can become ready a little after its
  hint. The quick retries after a hint last 20 s, which keeps the helpers few
  for a hint without a desktop, such as the X server of `xvfb-run`. Two cheap
  signals give the hints:
  - The XDG autostart entry runs
    `atuin desktop-capture login --in-desktop-session` once the desktop session
    is up, and the command then calls `SessionStarted` on a running daemon. The
    call is quiet and gives up after a second, because it runs on the login
    path. The login files don't report a session: a login on a text console
    comes before its compositor, for example before `exec sway`, and the watch
    sees the compositor once it runs.
  - The daemon watches for desktop sessions itself, every 2 s, without starting
    a helper. On the Unix systems other than macOS, the watch looks for a new
    display socket: a `wayland-<n>` or `gamescope-<n>` socket in the runtime
    directory that the user owns, or an `X<n>` socket in `/tmp/.X11-unix` that
    the user or root owns. In the runtime directory, a link to a socket counts
    as the socket, as WSLg's `wayland-0` is. In `/tmp/.X11-unix`, which every
    user can write to, only real sockets count. Root's sockets count because
    LightDM, SDDM on X11, GDM on X11, and `startx` through a root `Xorg.wrap`
    run the session's X server as root. Only root can create a socket that root
    owns, so other users can't fake one. A root X server that isn't the user's,
    such as a greeter's or another user's, costs one stretch of quick retries.
    The watch covers the sessions that don't run the autostart entry, such as
    sway started from a text console or a window manager that ignores XDG
    autostart. The daemon's environment usually lacks `WAYLAND_DISPLAY` and
    `DISPLAY`, so the watch doesn't rely on them. The watch doesn't see a
    compositor whose socket has another name. Such a session is seen through
    the autostart entry when the desktop runs it. On macOS, the watch checks
    every 2 s whether the user owns `/dev/console`. A helper whose user isn't
    at the screen counts as finding no desktop, at its start and when the user
    logs out or switches away.

  Where the session bus can start the accessibility bus on request, as
  systemd's user bus can, a helper connects even without a desktop session, and
  the applications of the next session join that accessibility bus. The wait
  for a session then happens only where the session itself starts the
  accessibility bus, or where the daemon doesn't have a session bus. A change
  of the settings or a start also brings back the quick retries. If a session
  is reported when a helper of the quick retries is running, the report applies
  to the wait after that helper. A session that is reported during the backoff
  after a failure ends the backoff, and the backoff starts over. A reported
  session doesn't change anything if a helper is capturing, if the helper lacks
  the permission, if the settings don't select any application, or if the watch
  on macOS sees that the user is away.

  A `start` ends every wait for a helper and starts a helper at once, with a
  new backoff. That covers the backoff after a failure, the minute before a
  helper without the permission starts again, the wait after a helper of
  another build, and the wait for a desktop session. The state shows `Starting`
  until that helper connects or ends. `atuin desktop-capture start` waits up to
  5 s for that outcome, so that it prints the state that the start led to.
  If a helper is capturing, a start only resumes a stopped capture. The
  supervisor reports one of the states `Disabled`, `NoApps`, `Stopped`,
  `Starting`, `Running`, `NeedsPermission`, `NoDesktop`, and `Failed`. A panic
  of the supervisor shows as `Failed`. A stop after the panic still writes the
  stop file, and `status` reports capture as stopped.
- **Stops.** A stop is kept in a small file in the store's directory, so that
  it lasts across daemon restarts. A directory that holds only the stop file
  doesn't count as a store, so a stop before capture ever ran doesn't make the
  daemon open a store or the MCP server list the desktop tools. The supervisor
  carries out each stop and start, and keeps the file, so the state and the
  file agree even when requests interleave. A stop takes effect before the file
  is written, and a start takes effect after the file is removed. The CLI
  writes the file itself only when it cannot reach the daemon, so that the stop
  holds once a daemon starts.
- **Storage.** Fjall keyspaces in `<data_dir>/desktop-capture/db-<n>/` hold the
  records, with the `HostId` and a UUIDv7 id on each. Most contents are stored
  as zstd deltas within their segment, as [Storing snapshots](storage.md)
  describes. A full-text index of the rendered text lives in
  `<data_dir>/desktop-capture-index.sqlite`, and a reconciler fills it from the
  store. The index notes the version of its schema in SQLite's `user_version`,
  and an open rebuilds an index of another version empty, so that the
  reconciler fills it again. The reconciler lists the contents of each batch of
  records, asks the index which ones it lacks, and expands only those. Each row
  of the index also holds its segment's kind, start, and application name in
  lowercase, so that a search applies those filters before it ranks, and an
  application filter ignores case. The index cannot tell when a segment ended,
  so the daemon reads the end from the store and takes more candidates when the
  store excludes some. A search removes the rows of the segments and the
  contents that the store no longer holds, and renders only the hits that it
  returns. The store opens whenever its directory holds a store, so it serves
  reads while capture is off. Nothing syncs.
- **gRPC.** The daemon's `DesktopCapture` service has `Status`, `Stop`,
  `Start`, `SessionStarted`, `RecentActivity`, `ListSegments`, `TallySegments`,
  `ListSnapshots`, `ReadSegment`, `ReadSnapshots`, `Search`, `Lookup`, and
  `Delete`. The commands, the MCP tools, and the prompt hook all go through it.
  `atuin desktop-capture login` calls `SessionStarted`. The daemon refuses
  calls from a client of another build, as it does for every service. Every
  call that takes an id takes the id of a segment or of one of its snapshots.
  `ReadSnapshots` renders several snapshots of one segment in one pass, so that
  a read of changes expands each content once. A request names at most 256
  snapshots, and the reply stops before the text that would take the texts past
  a total budget of at most 16 MiB. `TallySegments` counts segments per
  application for `stats` and for the preview of a deletion, and counts the
  damaged segments and records that a deletion would also remove. A deletion
  request names what it deletes: a filter, one id, or everything. The daemon
  refuses a request that doesn't.
- **Clients.** The `atuin desktop-capture` commands, the `atuin_desktop_*` MCP
  tools, and the `UserPromptSubmit` prompt hook are described in [the guide's
  reference](../guide.md#reference).

### Why fjall with a SQLite index

Three layouts were considered:

1. **Fjall holds every record, and SQLite holds only a full-text index that can
   be rebuilt.** This was chosen.
2. **Fjall for snapshot contents, SQLite for segments and observations.**
   Queries stay easy in SQL. There are two sources of truth to keep consistent,
   and sync would have to reconcile both.
3. **The prototype's SQLite store, moved as it is.** This was fastest to the
   demos, but it would have been rewritten before production.

Option 1 matches output capture, which already has a fjall store, an index, and
a reconciler to follow. Its records are immutable and keyed by time-ordered
ids, which is the shape that Atuin's append-only record store syncs. Fjall
compresses only with LZ4, one value at a time, so it does not see that a
snapshot repeats the previous one. The savings come from deltas that the daemon
makes itself.

### Platform constraints on the processes

- **The macOS permission belongs to the binary that asks for it.** The helper
  is the process that holds the Accessibility grant. The daemon spawns the
  helper as a plain child, and the helper spawns itself again with
  `responsibility_spawnattrs_setdisclaim`, through
  `atuin_common::os::macos::responsibility::respawn_disclaimed`, so that macOS
  charges the permission to the helper. The first process waits for the second
  and forwards SIGHUP, SIGINT, SIGQUIT, and SIGTERM to it. The second process
  watches the first through a kqueue and exits when the first exits, so a
  helper that the daemon kills leaves nothing behind. Every read of the
  desktop, including those for `setup` and `doctor`, happens in the helper.
- **The grant follows the code signature.** The helper keeps one stable
  identifier, `sh.atuin.desktop-capture`, so that a stable signature keeps the
  grant across rebuilds.
- **Linux capture should connect before applications start.** Qt and Firefox
  join the accessibility bus only when a client is listening. Chromium and
  Electron decide when they start. The daemon therefore has to start at login,
  and setup offers to arrange that. See [Setup on
  Linux](setup.md#setup-on-linux).
