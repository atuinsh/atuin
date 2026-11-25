# Test harness

Runs desktop capture against real applications in private desktop sessions, so
that a change can be checked under X11, Wayland, and macOS. Every scenario runs
capture through a private Atuin daemon. The unit tests in the crates cover the
logic of capture against a scripted backend. This harness covers what only real
toolkits and compositors show. [Rehearsing the demo in Claude
Code](../docs/development.md#rehearsing-the-demo-in-claude-code) walks through
the demo in a real Claude Code session, by hand.

## Linux

```sh
cargo build --release -p atuin --features desktop-capture
crates/atuin-desktop-capture/harness/session.sh <kind> <scenario>
```

Each run starts a private D-Bus session bus, a display server, and an
accessibility bus, with a fresh home directory under
`target/harness-run/<kind>` in the workspace. The scenario's checks are printed
at the end, as `ok`, `FAIL`, or `skip` lines, and `target/harness-run/<kind>`
keeps the daemon's settings, data, and logs, what the daemon stored, and the
output of the session. The run ends by signalling every process that is left in
its session, even when the run times out or is interrupted.

A run touches these paths outside `target/`:

- A runtime directory `/tmp/dch.XXXXXX`, which holds the sockets of the
  session, because the path of a socket must stay short. The run removes the
  directory at its end.
- With the `x11` kind, Xvfb's `/tmp/.X11-unix/X<n>` and `/tmp/.X<n>-lock`.
- With the `flatpak` scenario, the account's own Flatpak installation, from
  which the scenario runs Obsidian.

| Kind | Display server | Focus from scripts | Notifications |
|---|---|---|---|
| `x11` | Xvfb with xfwm4 | xdotool | dunst |
| `kwin` | KWin, headless, Wayland only | KWin scripting | dunst |
| `kwin-xwayland` | KWin with Xwayland | KWin scripting | dunst |
| `mutter` | mutter, headless | none | none |
| `gnome` | GNOME Shell, headless | the test extension in `gnome/` | GNOME Shell |
| `sway` | sway, headless | swaymsg | dunst |

Scenarios, in `scenarios/`:

- `smoke`: gedit and Kate open, and capture follows the focus between them.
- `segments`: GTK 3, Qt, and Firefox windows, GNOME Terminal and Konsole, an
  alert that closes after 300 ms, and a notification.
- `tabs`: Firefox's background tabs are swept and read when they change.
- `idle-firefox`: a static page in Firefox is not read again while Firefox's
  hidden New Tab page changes.
- `browsers`: a 12,000-element page in Chromium, a chat page, and VS Code when
  `VSCODE` names its `code` binary.
- `alerts`: ten alerts in a row, each for 300 ms after it appears, behind an
  active window.
- `qt-focus`: Chromium opens while Kate is running. Qt on Wayland sends an
  activation of its own window whenever another application takes the focus,
  which capture must not follow.
- `flatpak`: Obsidian from Flathub, started as `flatpak run` starts it, with
  `ACCESSIBILITY_ENABLED=1` in the session, and with the variable passed in.
  Capture must read Obsidian only with the variable, because the sandbox hides
  `org.a11y.Bus` from Obsidian. Needs
  `flatpak --user install flathub md.obsidian.Obsidian`, and skips otherwise.
- `spin-rows`: a libadwaita window with a switch row and two spin rows, which
  GTK 4 leaves out of the tree. No spin row is read at first. Once the last one
  takes the focus, it is read with its value, and the one that never had the
  focus stays missing. Needs `gir1.2-adw-1`, and skips otherwise.
- `blanks`: a page in Chromium whose blanks between words are text nodes
  between comments, as on chatgpt.com. Chromium leaves those blanks out, and
  capture must put them back. Spans that touch must stay one word.
- `terminals`: a page with a web terminal in the shape of xterm.js, in Chromium
  and Firefox, and VS Code's integrated terminal when `VSCODE` names its `code`
  binary. Capture must keep the page's text and a `[terminal]` line, and must
  not store any of the marker that each terminal holds, in the texts, the
  search index, or the data directory.
- `slow-read`: with a read budget of 150 ms, the page in Chromium is read in
  part and tried again three times.
- `daemon`: desktop capture through Atuin's daemon, end to end. It installs
  Atuin's Claude Code hooks, runs
  `atuin setup --only --desktop-capture --daemon`, starts `atuin daemon start`,
  and focuses gedit, Kate, and a stand-in for the WG14 proposal N3254 in
  Firefox. It checks `atuin desktop-capture log` and `status`, and pipes a
  recorded `UserPromptSubmit` event into the hook command that the Claude Code
  settings now hold. The context that the hook prints must name the page and
  Kate. After `atuin desktop-capture stop`, a change of focus must not add a
  segment.

  The scenario keeps everything in the session's work directory.
  `ATUIN_CONFIG_DIR` and `ATUIN_DATA_DIR` point there, the config gives the
  daemon its own socket and pidfile with `autostart = false`, and
  `CLAUDE_CONFIG_DIR` and `TMPDIR` point into the private home. The daemon
  finds the helper next to the `atuin` binary.
- `setup-apps`: what `atuin setup` writes to the `apps` setting. A config
  without `[desktop_capture]` gets `apps = "all"` and the four login files from
  `atuin setup --only --desktop-capture --daemon`, and the daemon then captures
  gedit without another command. `--desktop-apps listed` writes `apps = []`,
  which status reports as no application selected. A hand-written list survives
  a run without `--desktop-apps` and a run with `listed`, and
  `--desktop-apps all` replaces it. `atuin desktop-capture apps --write` then
  adds gedit to an empty list, and a running daemon captures gedit without a
  restart. A second run doesn't add anything. On `apps = "all"`, a run without
  a terminal leaves the configuration as it is, and `--write` fails without a
  change. A list with `"*"` fails with a hint that names `apps = "all"`, and
  `--no-desktop-capture` removes every login file that carries setup's marker.
  It keeps everything in the work directory, as `daemon` does.
- `keepon-toolkit`: `atuin setup --only --desktop-capture --daemon` with the
  `toolkit-accessibility` setting off, once in a home whose config already says
  `[desktop_capture] enabled = true`, as a config from a dotfiles repository
  does, and once with a config without the section. Both runs must write the
  login files and turn the key on, and a second run must not mention desktop
  capture. Needs a GSettings backend that keeps the key, as the `gnome` kind
  has, and skips otherwise.
- `private`: a normal and a private window in Firefox and Chromium, and in Edge
  and Brave when `EDGE` and `BRAVE` name their executables. Brave runs in
  German. Capture records the normal windows but not the private ones,
  and then, with `private_windows = "capture"`, the private ones too. Needs
  `TIMEOUT=400`.
- `stop-noapps`: `atuin desktop-capture stop` and `start` with `apps = []`.
  Status says that no application is selected, a stop adds the "Stopped" line,
  and a start takes the line away. gedit, added to `apps` while capture is
  stopped, stays uncaptured, and the stop survives a restart of the daemon. A
  start then captures gedit.

The packages on Debian:

```sh
apt install xvfb xfwm4 xdotool gnome-terminal konsole kwin-wayland mutter \
    gnome-shell sway wtype dunst at-spi2-core python3-gi gir1.2-atspi-2.0 \
    gir1.2-adw-1 zenity libnotify-bin gedit kate firefox-esr chromium
```

The harness runs `firefox-esr` when the system has it, and `firefox` otherwise.
`FIREFOX` names another Firefox.

The harness needs at-spi2-core 2.61 or later, which Debian 13 has in
`trixie-backports`. On 2.60 and earlier, the private home's
`toolkit-accessibility` is off, so Qt applications and Firefox never join the
bus, and the scenarios that read them fail.

`TIMEOUT` bounds a run in seconds, 120 by default. `ATUIN` names another
`atuin`, with its helper next to it, and `HARNESS_WORK` and `HARNESS_CACHE`
move the work and cache directories.

### Writing a scenario

A scenario is a Bash file that `lib/inner.sh` sources inside the session. The
header of that file lists the variables and helpers: `capture_start`,
`capture_stop`, `focus`, `type_text`, `firefox_`, `chromium_`, `mark`,
`records`, `expect`, and others. Applications run as native Wayland clients in
the Wayland kinds, and `on_x11` runs one through Xwayland.

`capture_start` and `capture_stop` run capture through Atuin's daemon. The
daemon's settings, data, and logs live in the session's work directory, and the
arguments of `capture_start` become lines of its `[desktop_capture]` settings,
such as `'apps = "all"'` or `'poll_interval = "0s"'`. When capture doesn't
start, `capture_start` prints a `FAIL` line and returns 1, so a scenario calls
it as `capture_start ... || return`. A scenario that writes its own
configuration takes the `[daemon]` and `[logs]` settings from `daemon_config`,
waits for the daemon with `wait_socket`, and waits for a state with
`status_is`. `mark` notes the time of each mark. After `capture_stop`,
`records` prints the stored segments and snapshots with the marks, in time
order, in the lines that `tools/records.py` describes. When capture is running,
`find_segment` finds a segment through `atuin desktop-capture search`, and
`atuin desktop-capture show` reads it.

### What looks like a bug in capture and is not

- **GTK 3 is active only with the keyboard focus.** A headless compositor
  without a keyboard never gives GTK 3 the focus, and gedit then never reports
  its window active. The harness adds a virtual keyboard: through mutter's
  RemoteDesktop API in mutter and GNOME Shell, and with `wtype` in sway. KWin
  has one of its own.
- **The session bus starts services with its own environment.** dconf,
  evolution-data-server, and the like inherit the home directory that was set
  when the bus started, so `session.sh` sets the home directory first.
- **The accessibility bus starts before its clients.** A client that starts
  first activates a second launcher, and capture then fails to connect.
- **A first start is slow.** A program that rebuilds its font cache takes
  seconds to show its first window, so the cache in `target/harness-cache`
  outlives the sessions.
- **The session bus starts services without the display.**
  gnome-terminal-server, for one, is started by the bus and exits when it
  doesn't find a display, so `lib/inner.sh` passes the display to the bus with
  `dbus-update-activation-environment`, as a desktop session does.
- **Flatpak apps installed with `--user` live in the account's own home**,
  which the session replaces with a private one. `session.sh` keeps the
  account's home in `HARNESS_USER_HOME`, and a scenario points
  `FLATPAK_USER_DIR` at `$HARNESS_USER_HOME/.local/share/flatpak`, as the
  `flatpak` scenario does.
- **sway's criteria are regular expressions**: `[title="notes (~)"]` does not
  match its own title. `focus` matches on the application id.
- **GNOME Shell starts in the overview**, where no window is active. The test
  extension hides it.
- **GNOME prevents focus stealing.** A window that a script opens may stay
  behind, so a scenario focuses each window that it means to be active.
- **A keyring prompt holds the keyboard.** Chromium and Electron ask the Secret
  Service for a keyring by default, and GNOME Shell's prompt then keeps the
  focus from every window. `chromium_` passes `--password-store=basic`.
- **KWin doesn't offer a virtual keyboard to clients**, so `type_text` is not
  available there, and `select_tab` switches tabs through their accessibility
  action in every session.
- **A socket's path holds at most 107 bytes.** sway, VS Code, and the daemon
  put their sockets in `XDG_RUNTIME_DIR`, so `session.sh` gives each run a
  runtime directory in `/tmp`, regardless of the path of the work directory.

## macOS

`macos/` holds two test applications and the scenarios that use them. Copy
`macos`, `lib`, `tools`, and `pages` of the harness to the Mac, into the
account that has the Accessibility permission for the helper, for example with
`rsync -a --delete macos lib tools pages <account>@<mac>:harness/`. Then, on
the Mac:

```sh
bash macos/build.sh            # builds ~/TwoWin.app and ~/Alone.app
bash macos/run.sh <scenario>   # lists the scenarios without arguments
```

`run.sh` runs capture through a private daemon, with the same `capture_start`,
`capture_stop`, and `records` as the Linux scenarios. `ATUIN` names the `atuin`
binary, and is the `atuin` on `PATH` by default. `HELPER` names the helper that
the daemon runs. The daemon has an Atuin home of its own in
`/tmp/desktop-capture-<scenario>`, which doesn't hold a signing certificate, so
the daemon runs `HELPER` as it is, and `HELPER` itself must hold the grant.
After `atuin setup` granted the permission, the signed copy of the helper in
`~/.atuin/desktop-capture/` holds the grant, so `HELPER` names that copy by
default. The copy's file name is in `~/.atuin/desktop-capture/signing.name`.
Without a signed copy, `HELPER` is the `atuin-desktop-capture` next to `ATUIN`.
The daemon's logs go to `/tmp/desktop-capture-<scenario>` too. `run.sh` builds
the test pages in `/tmp/desktop-capture-pages`. `records <app>` prints only the
segments of the applications whose names match, because a run reads every
window on the desktop. Stop any other capture in the account first, such as the
account's own daemon with `atuin desktop-capture stop`, so that it does not
read the same windows, and start it again afterwards.

The scenarios change the account's desktop: they quit Safari and TwoWin, and
`private` opens Firefox, Chrome, Edge, and Brave with profiles of their own in
`/tmp`. `private` needs the four browsers in `/Applications` or in
`~/Applications`. `terminals` opens Chrome with a profile of its own in `/tmp`
too. `private` also sets Brave's language to German for the run. When the run
ends, even by an error or a signal, `run.sh` puts the old language back, quits
the browsers that it started, and removes their profiles. The Firefox profile
gets `macos/firefox-user.js`, which turns off the onboarding that would cover
the toolbars of a new profile.

- `TwoWin` has two windows. "Background" gains a line every 3 seconds, and
  "Front" changes once after 20 seconds. Its modes minimize one or both
  windows, or put "Background" in full screen, which gives it a Space of its
  own.
- `Alone` shows only an alert for the given number of milliseconds, like a
  process without other windows.

`macos/axdrive.swift` drives applications through the AX API, so that tests
need neither Apple Events nor a person at the console. It presses buttons and
menu items, types into an application, posts keys, prints an application's
elements and their attributes, watches the AX notifications of an application,
and shows an alert through UserNotificationCenter. The comment at the top of
the file lists its commands. It needs the Accessibility permission, which macOS
ties to its path and to its signature, so build it with `swiftc`, sign it with
an identity that stays the same across builds, install it on `PATH`, and grant
it the permission there. Copy a new build to a new file and move it over the
old one: macOS kills a signed program whose file was rewritten in place.
`osascript` over SSH is no substitute, because it asks the person at the
console for permission to control each application.

A process that runs over SSH is not trusted for accessibility. The helper
relaunches itself with the responsibility of its parent disclaimed, so a daemon
that starts over SSH still runs a trusted helper, as long as the helper holds
the grant. Sign the helper with a stable identity as
`sh.atuin.desktop-capture`, so that the grant survives rebuilds. An account
that is logged in and locked at the console blocks every read in every session.
An account without the right to send Apple events can still run `open -a`.
