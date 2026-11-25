# Desktop capture: guide

Desktop capture records what the applications on your desktop show, so that an
agent such as Claude Code knows what you were just doing. For example, the
agent can see the error page that you looked at in the browser before you asked
it for a fix. Capture reads the screen through the accessibility APIs of Linux
and macOS, and it stores everything on your machine.

Desktop capture is an early preview without compatibility guarantees. A later
build may change its commands and settings, or require you to delete what it
stored. [Known gaps](known-gaps.md) lists what capture is known to miss.

## Quick start

1. Install the build with capture. You need Rust from
   [rustup](https://rustup.rs), and on macOS the Xcode command line tools. From
   a checkout of the branch, run:

   ```sh
   cargo install --path crates/atuin --locked --features desktop-capture
   ```

2. If mainline atuin is installed, such as from the install script or from
   Homebrew, take it off `PATH`. Otherwise that atuin replaces the daemon, and
   capture stops without a message:

   ```sh
   # atuin from the install script:
   mv ~/.atuin/bin/atuin ~/.atuin/bin/atuin.mainline
   # Homebrew's atuin, and its service if you started one:
   brew unlink atuin
   brew services stop atuin
   ```

   Then open a new terminal, and check that `which atuin` prints
   `~/.cargo/bin/atuin`. [Run only this atuin](#run-only-this-atuin) has the
   details.

3. Run `atuin setup`. Turn on desktop capture in the Desktop row, and turn on
   the Claude Code hooks and the MCP server, which are how Claude Code reads
   what capture stored.
   - On macOS, run setup at the Mac's screen, and grant the Accessibility
     permission on the page that setup opens.
   - On Linux, log out and log in again afterwards.

4. Check that capture is running. When it isn't, `status` says why and what to
   do:

   ```sh
   atuin desktop-capture status
   ```

5. Use your desktop for a minute, and then look at what capture stored:

   ```sh
   atuin desktop-capture log
   atuin desktop-capture search <words>
   ```

   In Claude Code, ask "what was I just looking at?"

To pause capture, run `atuin desktop-capture stop`, and `start` to resume it.
[Keep applications out](#keep-applications-out) and [Turn capture
off](#turn-capture-off) cover the rest of the controls.

The rest of this guide explains each step in more detail. [The
reference](#reference) at the end lists the commands, the MCP tools, the prompt
hook, and the settings.

## Go back to mainline atuin

Mainline atuin is any atuin without desktop capture. For example, it can be an
atuin from the install script, from Homebrew, or from your own build of `main`.

1. Run `atuin setup --only --no-desktop-capture` with the atuin that has
   capture. The command removes what setup wrote for capture, including the
   prompt hook.
2. Restore mainline atuin. Run `mv ~/.atuin/bin/atuin.mainline
   ~/.atuin/bin/atuin` or `brew link atuin`, and then `cargo uninstall atuin`.
   For your own build, run `cargo install` from its checkout instead.
3. Run mainline atuin's `atuin setup` again, from a terminal where `claude` is
   on `PATH`. Check with `claude mcp get atuin` that Claude Code's MCP entry
   names mainline atuin.

If you skip step 1, remove the `UserPromptSubmit` hook from
`~/.claude/settings.json` by hand, because mainline atuin doesn't know that
hook.

## Install

The repository pins its toolchain, so `cargo install` fetches the right Rust
version by itself. `xcode-select --install` installs the Xcode command line
tools on macOS.

`cargo install` puts two binaries into `~/.cargo/bin`: `atuin`, and the capture
helper `atuin-desktop-capture`. The daemon looks for the helper next to its own
executable, so keep the two binaries together. The command may print `default
toolchain implicitly overridden` and `patch axoasset … was not used`, which are
both harmless.

To update, pull the branch and run the same `cargo install` again. The next
`atuin` command replaces the daemon of the old build. On macOS, the new helper
keeps the Accessibility permission.

### Run only this atuin

Your shells, their hooks, and the daemon must all run the atuin in
`~/.cargo/bin`. Each atuin replaces a daemon of another build, and the daemon
of mainline atuin doesn't run capture. One old terminal or tmux pane that still
runs mainline atuin is enough to stop capture. On macOS, each replacement also
shows a "Background Items Added" notification.

Mainline atuin often comes first on `PATH`, even after `cargo install`.
The install script adds `~/.atuin/bin` to `PATH` in your shell's startup files.
On macOS, rustup's default setup puts `~/.cargo/bin` last.

To switch to this atuin:

1. Take the other atuin off `PATH`. For atuin from the install script, rename
   it with `mv ~/.atuin/bin/atuin ~/.atuin/bin/atuin.mainline`. For Homebrew's
   atuin, run `brew unlink atuin`, which keeps the package installed. If you
   ran `brew services start atuin`, also run `brew services stop atuin`,
   because that service runs Homebrew's atuin regardless of `PATH`.
2. Put `~/.cargo/bin` first on `PATH`. Add this line to your shell's startup
   file, after the line that sets up the other atuin and before `atuin init`:

   ```sh
   export PATH="$HOME/.cargo/bin:$PATH"
   ```

   In fish, run `fish_add_path --move ~/.cargo/bin` once instead.
3. Close every terminal, tmux session, and editor terminal that was open before
   the change.
4. Open a new terminal, and check that the first line of `which -a atuin` is
   `~/.cargo/bin/atuin`. Then run `atuin setup`, which moves the daemon, its
   launch agent or login files, and the hooks to this atuin.

Atuin warns you when another atuin is in the way. `atuin setup`,
`atuin desktop-capture status`, and `atuin doctor` each name the other atuin
and the command that fixes the problem. If `status` itself fails with "needs an
`atuin` with desktop capture", mainline atuin is still first on `PATH`.

## Turn capture on

Run `atuin setup` and turn on desktop capture in the Desktop row. Setup
recommends "Every application", which writes `apps = "all"`. "Only the
applications I list" starts with an empty list, so that capture stays idle
until `atuin desktop-capture apps` fills the list.

To turn on only capture without the menu, run:

```sh
atuin setup --only --desktop-capture --daemon
```

### On macOS

Run `atuin setup` in a terminal on the Mac itself, because the permission needs
you at the screen. Setup refuses to turn capture on over SSH.

Setup ends with a page that opens the Accessibility pane of System Settings.
Turn on `atuin-desktop-capture (<your user name>)` there. If a "Background
Items Added" notification covers the switch, close the notification first.
Capture starts as soon as setup notices the grant. If you press esc to grant
the permission later, run `atuin setup` again when you are ready.

If you turned capture off before, the pane may show the entry as on while setup
keeps waiting. That entry belongs to an old certificate. Remove it with the
minus button, and turn on the new entry that appears within a minute.

Capture pauses while the screen is locked, and it never stores the lock screen.

### On Linux

Setup changes the files that your sessions read at login, so that capture
starts at each login. It also turns on GNOME's `toolkit-accessibility` setting,
which Qt applications and Firefox need on most distributions.

Log out and log in again after setup, so that Chromium, Chrome, and Electron
applications expose their content. An application that was already running may
need a restart. `atuin desktop-capture apps` tells you which ones.

### VS Code

VS Code exposes the text of its editor only with this line in its
`settings.json`, followed by a restart:

```json
"editor.accessibilitySupport": "on"
```

Capture never reads VS Code's terminal panel.

## What capture stores

Capture stores five kinds of records:

- **Activity**: which application and window was active, with the window's
  title and the page's URL, from when to when.
- **The active window's text**, read again whenever the window changes. A web
  page is read whole, including the parts outside the view.
- **Dialogs**, as soon as they appear, even ones that you dismissed at once.
- **Notifications**: the sending application, the title, and the body.
- **Background windows**, read when capture starts and again when they change.

Capture leaves these out:

- Terminals, because Atuin's history and output capture cover them. A terminal
  inside another window, such as an editor's terminal panel, shows as a
  `[terminal]` line.
- Password fields, and the browser's address bar.
- The private windows of Firefox, Safari, Chrome, Chromium, Edge, and Brave.
- The query part of URLs, and the credentials in URLs.
- Credentials and one-time codes that match Atuin's patterns. The matching is
  best-effort.

Everything stays in `~/.atuin/data/`, and Atuin sync never uploads it. The
store uses at most 10% of the disk by default, and it deletes the oldest
records past that limit. `max_disk_usage` in the
[configuration](../../../docs/docs/configuration/config.md#desktop-capture)
changes the limit.

What capture stored can leave your machine through an agent:

- With Atuin's Claude Code hooks, each prompt carries a list of the windows,
  dialogs, and notifications of the last 10 minutes. The list holds their
  titles and URLs, and the message of each dialog. It doesn't hold the text of
  the windows.
- With the MCP server, an agent can search and read the text of every captured
  window. Claude Code asks you before it calls a tool, unless you allowed the
  tool.

To look at what capture stored:

```sh
atuin desktop-capture log                         # recent windows, with ids
atuin desktop-capture log --kind notification     # recent notifications
atuin desktop-capture search deploy failed        # windows with these words
atuin desktop-capture show <id>                   # one window's text
atuin desktop-capture stats                       # size, per application
```

## Keep applications out

Add the applications that capture should never read to `exclude` in the
`[desktop_capture]` section of Atuin's config file. Use the names that
`atuin desktop-capture log` shows, in any case:

```toml
[desktop_capture]
exclude = ["KeePassXC", "Signal"]
```

The config file is `~/.atuin/config.toml` if that file exists, and
`~/.config/atuin/config.toml` otherwise. The running daemon applies the change
at once.

To capture only the applications that you name, run
`atuin desktop-capture apps`, which offers to replace `apps = "all"` with a
list.

To pause capture, run `atuin desktop-capture stop`. Capture stays off, across
restarts too, until `atuin desktop-capture start`.

## Delete what capture stored

```sh
atuin desktop-capture delete --app firefox            # one application
atuin desktop-capture delete --after 2h               # the last two hours
atuin desktop-capture delete --after 3h --before 1h   # a time range
atuin desktop-capture delete --id <id>                # one window
atuin desktop-capture delete --all                    # everything
```

The command shows what matches and asks before it deletes. Capture then reads
every open window again, so close the windows that you want gone first, or run
`atuin desktop-capture stop` before you delete.

The deletion covers only capture's own store. Claude Code's transcripts,
Atuin's agent threads, and Atuin's terminal output keep what they recorded.

## Turn capture off

```sh
atuin setup --only --no-desktop-capture
atuin desktop-capture delete --all   # optional: delete what capture stored
```

Turn capture off before you delete, because capture would otherwise read the
open windows again.

Setup stops the helper, takes the prompt hook out of Claude Code, and undoes
what it wrote for capture. On macOS, you can remove the entry in the
Accessibility pane with the minus button. Turn capture off before you switch to
an atuin without capture, because only a build with capture can undo its setup.

## When an application reads as empty

Some applications expose little or nothing to the accessibility APIs. Try these
steps:

1. Run `atuin desktop-capture status`, and check that capture is running.
2. Restart the application. Many applications decide at their start whether to
   expose their content.
3. Run `atuin desktop-capture doctor`, which lists what each running
   application exposes.
4. Check the [known gaps](known-gaps.md).

To report the problem, include these details:

- Your OS and its version. On Linux, also your desktop, and whether the session
  runs Wayland or X11. `echo $XDG_SESSION_TYPE` prints which one.
- The application's name and version, and how you installed it.
- What you expected capture to read.
- The output of `atuin desktop-capture doctor` and of
  `atuin desktop-capture doctor --outline <application>`.
- The `desktop_capture` section of `atuin doctor`.

`doctor` and `--outline` don't print any text of your windows. `atuin doctor`
prints your configuration and paths, and `show` and `search` print the content
of your windows, so check their output before you pass it on.

## Reference

### Commands

- `status` shows whether capture is running and how much the store holds. When
  capture isn't running, `status` says why and what to do.
- `stop` pauses capture until `start`, across daemon restarts and
  `atuin setup`. `start` resumes capture, and it also retries at once after a
  failure. `start` waits up to 5 seconds and prints the state that capture
  reached.
- `log` lists the most recent segments with their ids. `--kind` keeps one kind
  and can be repeated. The kinds are `active`, `dialog`, `background`, and
  `notification`. `--limit` caps the list.
- `search <words>` searches the text, applications, and titles of the captured
  windows and notifications. It takes these filters:
  - `--kind`, as for `log`.
  - `--app`, by the name that the hits show, ignoring case.
  - `--after` and `--before`. They take a duration such as `2h`, an RFC 3339
    time, or a date such as `yesterday`.
  - `--style`, which is `plain`, `pretty`, `json`, or `ndjson`.
- `delete` deletes the segments that `--app`, `--after`, and `--before` select.
  `--all` deletes everything, and `--id <id>` deletes one segment. The command
  asks first, and `--yes` skips the question. The deleted content doesn't
  remain in the files of the store or of the search index.
- `show <id>` prints the latest snapshot of a segment, or the snapshot with
  that id. `--first` prints the first snapshot, and `--snapshot <id>` picks
  another snapshot.
- `snapshots <id>` lists every snapshot of a segment, oldest first.
- `stats` totals the store, and each application's segments of the last 24
  hours.
- `apps` finds the applications that expose content, and asks for a restart of
  each application that needs one. Then the command offers to write those
  applications into `apps`. `--write` and `--no-write` answer the question.
- `doctor` lists the running applications and what each one exposes.
  `--outline <app>` prints the role tree of the application's windows without
  their text.

### MCP tools

`atuin setup --only --mcp --agents claude-code` connects Claude Code to Atuin's
MCP server. Once the store exists, the server lists three more tools:

- `atuin_desktop_activity` lists the windows, dialogs, and notifications of a
  time range.
- `atuin_desktop_read` reads a segment or a snapshot. With `"changes"`, it
  lists what changed between the snapshots of a segment. For example, an agent
  can see an option that the user toggled in a settings window.
- `atuin_desktop_search` searches the text of every captured window, with the
  same filters as the `search` command.

The tools use the time zone of the `timezone` setting.

### The prompt hook for Claude Code

`atuin hook claude-code` handles the `UserPromptSubmit` event. It adds one line
to the prompt for each window, dialog, and notification since the session's
last prompt, with its time, application, title, and URL. The hook goes back at
most `hook_minutes`, and it leaves out windows that stayed in the background.
The hook waits at most 300 ms for the daemon, and it doesn't add anything when
capture isn't running.

`atuin setup` registers the hook when both desktop capture and Atuin's Claude
Code hooks are on, in either order.

### Settings

The `[desktop_capture]` section of Atuin's config file holds the settings,
which [the configuration
reference](../../../docs/docs/configuration/config.md#desktop-capture)
describes one by one. Capture doesn't run when `apps` is empty, even if
`enabled` is true.

The running daemon applies a change to the section as soon as the file changes.
`max_disk_usage` is the exception, which takes effect when the daemon restarts.
