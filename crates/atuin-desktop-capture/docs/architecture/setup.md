# Desktop capture: setup

What `atuin setup` and `atuin desktop-capture apps` do to prepare a machine for
capture, on every platform, on Linux, and on macOS. [What each environment
provides](environments.md) collects the findings that these steps are based on.

## Setup on every platform

Desktop capture is a choice of `atuin setup`. Builds without the daemon, or for
a platform other than Linux and macOS, don't offer it. Turning capture on
writes `desktop_capture.enabled`, prepares the platform as the next two
sections describe, starts the daemon, and registers the prompt hook for Claude
Code. Turning capture off removes what setup wrote, including the prompt hook,
and keeps Atuin's other Claude Code hooks. `--no-desktop-capture` removes these
files even when capture is already off. On macOS, turning capture off also
removes the certificate, the keychain and its entry in the search list, the
signed copy of the helper, and the leftovers of an interrupted creation or
signing. The first signed helper that a process asks for also removes the
temporary copies that the signings of ended processes left behind, such as a
daemon that was killed during a copy. The helper's row in System Settings
stays, so turning capture on again needs a new grant.

Neither turning capture on nor turning capture off removes the stop file of
`atuin desktop-capture stop`, so a stop still holds after setup turns capture
on. When capture ends up on and a stop holds, setup says so in its summary and
names `atuin desktop-capture start`. The interactive setup also offers to start
capture on a page of its own, before the permission page. `--yes` doesn't offer
the start. Setup asks a daemon that answers whether a stop holds, as
`atuin desktop-capture status` does, and reads the stop file when no daemon
answers. Setup doesn't start a daemon to ask.

`atuin desktop-capture apps` writes its selection through a pipe. `find_apps`
in `crates/atuin/src/command/client/desktop_capture/setup.rs` passes the helper
the pipe and names its file descriptor in `ATUIN_DESKTOP_CAPTURE_APPS_FD`. The
helper opens `/dev/fd/<n>` and writes one `AppsRecord` in JSON. `atuin` stops
reading once the helper exits, and on macOS the pipe passes through the
helper's disclaimed respawn. `atuin` then shows the list and the difference,
asks, and writes through `update_config_file`. A list gets the union of its
names and the new ones. Names that differ only in case count as one, and the
names that `exclude` names stay out. A running daemon applies the new list
without a restart. The suggestion leaves out the applications that
`Inspect::shows_only_prompts` names, which each platform's `quirks` lists:
`SecurityAgent`, the permission alert, and `pinentry-mac` on macOS, and
`kwalletd5`, `kwalletd6`, `gcr-prompter`, the `polkit-*` agents, `lxpolkit`,
and `pinentry*` on Linux. It also leaves out what `Backend::is_left_out` names,
as capture does. The harness's `setup-apps` scenario covers the writes.

Setup holds the other end of the stdin of the helper that waits for the
permission, and `access request --until-stdin-closes` exits once stdin closes.
A setup that a signal or `kill -9` ends therefore stops the helper as well. The
permission page also treats Ctrl-C as "later".

`atuin doctor` has a `desktop_capture` section with the capture settings, the
build of `atuin`, the supervisor's state with its error, and the helper's path
and whether the helper exists. On macOS, the section says whether the helper
has the permission and whether the certificate exists. On Linux, it holds the
session variables of the process. The report doesn't start a daemon or sign a
new copy of the helper.

## Setup on Linux

`atuin setup` arranges the logins and `toolkit-accessibility`, and
`atuin desktop-capture apps` runs the helper's `apps` command. The design of
both follows from how toolkit accessibility is switched.

Qt applications and Firefox join the accessibility bus only when the session
flag `org.a11y.Status.IsEnabled` is on, and they follow its changes. What turns
the flag on depends on the version of at-spi2-core:

- **2.61 and later.** Fedora 45, Arch, and openSUSE Tumbleweed ship it as of
  November 2026. The bus launcher turns the flag on when any client is
  listening for events, and turns it off when the last listener leaves. A
  capture process that stays connected therefore keeps the flag on. The
  GSettings key `org.gnome.desktop.interface toolkit-accessibility` doesn't
  affect the flag. The first listener writes `true` to the key anyway, and
  nothing writes it back. A flag that a client sets by hand lasts until the
  last listener leaves.
- **2.60 and earlier.** Every LTS distribution ships it, including Ubuntu 26.04
  and Debian 13, and so does Fedora 44. The flag mirrors
  `toolkit-accessibility` in both directions, and a listener doesn't change
  anything. Without the key, Qt applications, Firefox, and the whole Plasma
  desktop never join, even with capture connected. Setting the key, or writing
  the flag, which writes the key, makes every later session start with the flag
  on, because dconf keeps the key.

An application that was already running joins the bus after the flag turns on.
In tests, Kate and Firefox became fully readable within one second, and a read
during that moment sees a partial tree. Before setup judges an application, the
backend's `wake_apps` therefore waits 3 seconds when connecting turned the flag
on, and at least a second when a window embeds a document whose page may still
be building. Firefox acts on the change only when one of its windows takes the
focus, so a Firefox behind other windows or behind the lock screen stays off
the bus until the user switches to it.

Chromium, Chrome, and every Electron application decide once, at their start,
whether to join the accessibility bus. The first of `ACCESSIBILITY_ENABLED`,
`GNOME_ACCESSIBILITY`, and `QT_ACCESSIBILITY` that is set to `1` or `0`
decides. Without any of them, the application joins when the flag is on at that
moment. With `ACCESSIBILITY_ENABLED=1`, the order of starting no longer
matters. Debian and Ubuntu set `QT_ACCESSIBILITY=1` for every session through
`/etc/environment.d/90qt-a11y.conf`, which is why Chromium and Electron join
there in every case. Qt itself does not read `QT_ACCESSIBILITY`. Qt 5.15 and Qt
6 read only the flag and `QT_LINUX_ACCESSIBILITY_ALWAYS_ON`.

The helper's `apps` command runs six steps. Each step reports what it found and
what the user still has to do.

1. **Permission.** The command asks the platform whether the helper has the
   permission, in the helper's own process. Linux doesn't need a permission, so
   this step reports it as granted. On macOS, a missing permission stops the
   command with "run `atuin setup`", because only `atuin setup` requests the
   permission.
2. **Accessibility service.** The session bus must offer `org.a11y.Bus`. This
   needs `at-spi2-core`, which desktop distributions install by default. If it
   is missing, setup names the package and stops.
3. **Session environment.** Setup reports whether this session runs with
   `ACCESSIBILITY_ENABLED=1`. An application has the variable only when the
   session that starts the application passes the variable on. `atuin setup`
   puts the variable into every session, as the paragraphs below describe, so a
   session has the variable from the next login on. The variable is a safety
   net on at-spi2-core 2.61 and later, where a capture process that starts
   early in the session already makes every later application join. On 2.60 and
   earlier, the variable reaches Chromium and Electron only, and Qt and Firefox
   need `toolkit-accessibility`, which `atuin setup` sets.
4. **Running applications.** Setup lists the browsers and Electron applications
   that run but are not on the bus, and asks the user to restart each. Chromium
   needs this when it started before any client. A Firefox that started before
   any client joins only when one of its windows takes the focus, which looked
   like a need to restart it. Setup stays connected from here on, so a client
   is there when the application comes back.
5. **Result.** Setup prints the report of `doctor`: each application on the bus
   with the amount of readable content.
6. **Applications to capture.** The helper writes the applications that exposed
   content to a pipe that `atuin` passes it, apart from the system prompt
   agents. `atuin` adds them to the `apps` list after it asks, so the
   configuration keeps the applications that an earlier run found. A helper
   that runs by hand prints an `apps = [...]` line instead. `atuin setup`
   writes `apps = "all"` when the user picks "Every application", and an empty
   list otherwise.

`atuin setup` prepares every session that the user can log in to, from the
machine and the user's account rather than from the session that it runs in.
[The files that each session
reads](environments.md#the-files-that-each-session-reads) has the facts behind
each file:

- One marked line in `~/.profile`, in the login shell's own file, and on Debian
  in `~/.xsessionrc`. The login shell comes from the password database. For
  Bash, its file is the first of `~/.bash_profile` and `~/.bash_login` that
  exists. For Zsh, it is `.zprofile` in the `$ZDOTDIR` that `~/.zshenv` leaves.
  Setup asks `zsh` for the directory, which `zsh` prints after a marker,
  because `.zshenv` can print text of its own. When `zsh` doesn't answer, setup
  writes `~/.zprofile` and notes that `ZDOTDIR` may name another directory. For
  fish, it is `atuin-desktop-capture.fish` in fish's `conf.d`, and for Nushell,
  `login.nu`. For another shell, setup prints the line to add.
- `~/.config/environment.d/60-atuin-desktop-capture.conf` with
  `ACCESSIBILITY_ENABLED=1` and `NO_AT_BRIDGE=0`, when systemd is the init
  system. It reaches the Wayland sessions that GDM starts without a shell.
- `~/.config/autostart/atuin-desktop-capture.desktop`, for the desktops that
  run XDG autostart entries. This entry and the files of fish and Nushell
  follow the `XDG_CONFIG_HOME` of the session that runs setup. When that
  variable names a directory other than `~/.config`, and no login file of the
  user or the system seems to set it, setup notes the files that it wrote
  there, because a login that doesn't set the variable looks in `~/.config`.

Each line and the autostart entry run the hidden `atuin desktop-capture login`
through the path that `launch_path` gives, with the variables that locate
Atuin's configuration and data. The command starts the daemon when the daemon
isn't running, prints the session variables in the syntax of the shell, and
rewrites the file in `environment.d` when it is out of date. The autostart
entry passes `--in-desktop-session`, because the desktop runs the entry inside
a desktop session that exists by then. With that flag, the command tells a
daemon that is already running that a desktop session started, so that capture
resumes at once when the daemon is waiting for one. The command doesn't do
anything when capture is off. Applications that start while capture is
connected see the accessibility flag on from their first moment, so capture
starts with the session. Setup also sets `toolkit-accessibility` and records
the old value, and it warns about variables that hide applications, such as
`GTK_A11Y=none`. It lists all of its changes and asks once. After the helper's
steps, it registers the Claude Code prompt hook when Atuin's Claude Code hooks
are installed.

Setup appends its marked line to a file, after a blank line, so that `login`
and the daemon that it starts see what the rest of the file sets, such as
`XDG_CONFIG_HOME` or `PATH`. A line that replaces the shell would keep an
appended line from running. For example, an Arch `~/.bash_profile` often ends
with `exec sway`. In a file of `sh` with an `exec` command, setup puts the line
before the top-level statement that holds the first `exec`, and before the
comments directly above that statement, followed by a blank line. Setup inserts
the line there only when the scan read the nesting of the whole file without a
doubt, because a line inside a compound command can break the login. For
example, a line before `then exec sway` joins the condition of the `if`, so the
`exec` runs on every console, and a line before a `case` arm is a syntax error.
The scan doubts when it ends with a compound command still open, when a keyword
such as `then` or `done` doesn't fit the innermost compound command, or when it
meets a compound command that it doesn't follow, such as zsh's `repeat`. The
scan also doubts when a quote, a bracket, or an arithmetic command doesn't end.
The scan reads the patterns of a `case` as patterns, so reserved words such as
`done|fi)` and `(esac)` don't make it doubt. It reads `[[ … ]]` as a compound
command whose words don't start commands, so a test can span lines and hold
words such as `(for)`. It reads the commands of each `$(…)` as it reads the
file, so a `case` inside `$(…)` doesn't end the substitution early, and a doubt
inside `$(…)` is a doubt of the file. A misread can hide an `exec` from the
scan, so in a file that the scan doubts, the first line that holds a word
`exec` before a program counts as the line of the `exec`. In a file that the
scan doubts, setup appends the line at the end and reports that the user has to
move the block above the `exec`. A later run leaves the block where the user
moved it. Removal takes out the blank line that setup added with the marked
line. Nushell's `login.nu` always gets the line at the end, and setup reports
that the user has to move the block when `login.nu` holds a word `exec`. A test
runs the login files of `crates/atuin/tests/login_files` in `bash`, `zsh`, and
`dash`, before and after setup adds its line, and checks that the edited file
runs as the original does.

Setup edits the user's files as bytes, so a file in another encoding, such as a
Latin-1 `~/.profile`, gets the line too. Each new content goes to a temporary
file in the same directory, which then replaces the file at once and keeps its
permissions. Setup writes through a symbolic link, so the link of a dotfile
manager stays a link. A file that changed after setup read it, such as while
setup waited for the answer, stays as it is, and setup reports it. A file that
setup cannot read or write leaves the other files to go on. A line after the
marker that the user commented out belongs to the user, and setup adds its own
block next to it. Each run of setup that leaves capture on does every step that
turning capture on does. It writes the whole set of files again, puts back a
block that the user took out, and turns `toolkit-accessibility` on when the key
is off. Such a run keeps `Hidden=true` and `X-GNOME-Autostart-enabled=false`,
with which a desktop turns the autostart entry off, and reports only what it
changed. A run that turns capture on turns the entry back on.

Setup creates the directories that its files need with the permissions that the
user's umask leaves, and notes each directory that it created in Atuin's
`desktop-capture` directory. Removal takes out the noted directories that are
empty once setup's files are gone. `atuin desktop-capture login` rewrites only
the file in `environment.d`. Removal keeps `~/.bash_profile` and
`~/.bash_login` even when they end up empty, because an empty file still keeps
Bash from reading the files after it.

`atuin desktop-capture doctor --outline <app>` prints the structure that an
application exposes, without any of its text, which makes a low count
diagnosable without sharing content.

### What setup handles

[What each environment provides](environments.md#linux) collects what each
toolkit, distribution, and display manager provides, and which logins confirmed
it. From those facts, setup and `doctor` handle these things:

- **`toolkit-accessibility` on 2.60 and earlier.** It is the only switch that
  makes Qt and Firefox readable there. It persists in dconf, so setup records
  the old value, and turning capture off puts that value back. Turning capture
  on in `atuin setup` sets the key without asking, and the summary of setup
  says that setup turned the key on. Without dconf as the GSettings backend,
  the key does not persist. Setup then leaves the key alone and says so, and Qt
  and Firefox stay off the bus there.
- **The session variable `ACCESSIBILITY_ENABLED=1`** in every file that some
  session of the user reads, for Chromium and Electron that start before
  capture, and for Flatpak. Setup writes the files that [Setup on
  Linux](#setup-on-linux) lists.
- **Variables that turn applications off:** `NO_AT_BRIDGE=1`, `GTK_A11Y=none`,
  and any of `ACCESSIBILITY_ENABLED`, `GNOME_ACCESSIBILITY`, or
  `QT_ACCESSIBILITY` set to `0`. Setup's files set `ACCESSIBILITY_ENABLED=1`
  and `NO_AT_BRIDGE=0`. Setup says that it overrides those two where every
  session reads its files later: in `/etc/sway/env`, and in a file of
  `environment.d` whose name comes before setup's file. It warns about each
  other hiding variable that it finds. That includes those two in
  `/etc/environment`, which systemd links into `environment.d` as
  `99-environment.conf`, after setup's file. `apps` and `doctor` say when their
  own session has a hiding variable.
- **Which flag behaviour the bus has.** Setup sets `toolkit-accessibility`
  without a test of the flag's behaviour, because the key doesn't change
  anything on 2.61 and later.
- **Qt 5 applications** are best effort on X11 and unreadable on Wayland. [Qt 5
  on X11](environments.md#qt-5-on-x11) explains the race on X11. `doctor` lists
  Qt 5 applications as it lists every other application.

`GNOME_ACCESSIBILITY=1` and `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1` would make
Firefox and Qt join without the flag. Setup leaves them out, because the flag
covers both, and Firefox builds its tree for every page as soon as
accessibility is on. KDE's `plasma-browser-integration-host`, which Firefox and
Chromium spawn, registers as a Qt application without windows under the
browser's name.

### Chromium and Electron don't need a launch switch

Chromium and Electron applications read fully without
`--force-renderer-accessibility`. Chromium's source shows two separate
decisions:

- **Joining the bus** is decided at the start: by `ACCESSIBILITY_ENABLED`,
  `GNOME_ACCESSIBILITY`, or `QT_ACCESSIBILITY` in the environment, or else by
  `org.a11y.Status.IsEnabled` at that moment. A Chromium that started without
  either never joined in the tests, even after clicks and key presses in its
  window.
- **Building the tree of its pages** is decided on demand. Chromium takes a
  request for the relations or for the attributes of any of its elements as the
  sign that a client wants the pages, because the Orca screen reader makes
  those calls. The hook has been in Chromium since version 100 at least, so
  current Electron applications have it.

The Linux backend asks every window for its relations at each read, to find the
document that the window shows. That request is the trigger. The tree arrives a
moment later, so capture waits for it. A window that names a document that the
read did not meet counts as still loading, regardless of the application. On
the very first read, Chromium has not named the document yet. A window without
any document therefore counts as loading for the second after the backend's
first request for relations to its application, regardless of the toolkit.

Chromium doesn't have another switch. It looks for a process named `orca` to
label the assistive technology, but a process with that name does not turn the
web content on. Setting `ScreenReaderEnabled` on the accessibility bus does not
either. Both were tried with Chromium 156.

Verified with Chromium 156 and VS Code 1.145, both started without any switch:

| Case | Result |
|---|---|
| A client connected first | First read: the browser's own bars. Second read: the whole page. |
| Capture running | One wait for the page, then a snapshot of the whole page. |
| Started before any client | Never on the bus. Setup asks for a restart. |
| Started before any client, with the session variable | On the bus, and readable. |

The page of a tab that stays hidden for five minutes loses its tree, and gets
it back when the tab shows again.

In a Flatpak sandbox, the variable is the only way in. Obsidian 1.15 from
Flathub never joined the bus as `flatpak run` started Obsidian, even with
capture connected before the start. The sandbox's D-Bus proxy lets Obsidian
talk only to the names in its manifest. `org.a11y.Bus` is not among those
names, so Obsidian cannot read `IsEnabled`. With `ACCESSIBILITY_ENABLED=1` in
the environment of the program that started it, as a session has it after
setup, `flatpak run` passed the variable on, Obsidian joined, and capture read
its window. Setup finds such an application through `/proc`, where
`/.flatpak-info` in the root of the process marks the sandbox, and asks for a
restart once the session has the variable. Some of Flatpak's `bwrap` launchers
run Chromium's helpers and carry their arguments, so an application is found
only when its helpers run below it, and of two that run one inside the other,
only the inner one counts.

## Setup on macOS

`atuin setup` implements the user-facing part of these steps, and
`atuin desktop-capture apps` the last one. Setup turns capture on only for the
user at the Mac's screen. Away from the screen, such as over SSH, setup would
start the daemon without a launch agent, so nothing would start capture after a
restart, and the permission page could not open System Settings. The Desktop
row therefore says that it needs setup at the screen, and `--desktop-capture`
fails with the same reason. A user can still turn capture off, or keep it on,
from anywhere. [What each environment provides](environments.md#macos) lists
the runs on macOS 13 to 27 that checked these steps.

1. **Sign the helper with a certificate that stays the same.** The
   Accessibility permission is tied to the code signature. Atuin's release
   binaries are only ad-hoc signed, so their signature is a build hash, and
   every update would silently revoke the permission. Setup therefore creates,
   without asking, a self-signed certificate in a keychain of Atuin's own,
   `~/.atuin/desktop-capture/signing.keychain-db`. Once the certificate exists,
   Atuin runs a copy of the helper from that directory. Before the daemon or
   the CLI starts the helper, the daemon or the CLI signs a new copy when the
   helper next to `atuin` or the certificate has changed. The copy's
   requirement is
   `identifier "sh.atuin.desktop-capture" and certificate leaf = …`, which
   stays the same across builds, so one grant lasts through every update. The
   copy also keeps the path of the grant the same, even though Homebrew's
   Cellar path changes with every version. A Developer ID signature would keep
   the grant the same way, without a certificate on the user's machine.

   The key cannot be exported, and only `codesign` may use it without a prompt.
   A password file next to the keychain unlocks the keychain, because macOS
   locks the keychain again when the session ends. On macOS 14,
   `codesign --keychain` finds the certificate only in a keychain on the user's
   search list, so setup and the daemon add the keychain to that list. Any
   program that runs as the user can read the password and sign itself with the
   certificate, and it would then inherit the grant. The copy is signed with
   the hardened runtime, which keeps other code from loading into the granted
   helper.

   A new certificate names the signed copy after the user, as in
   `atuin-desktop-capture (admin)`, so that the rows of different accounts in
   the Accessibility pane can be told apart. The name is stored in
   `signing.name`, and a certificate without that file keeps the plain name, so
   that earlier grants still hold. The name stays when the user is renamed, and
   the next certificate takes the new one.
2. **Launch the helper so that the permission attaches to it.** macOS charges a
   permission to the process that launched the requester. A helper that the
   daemon spawns would be charged to the daemon, or to the terminal that
   started the daemon. The helper therefore spawns itself again with
   `responsibility_spawnattrs_setdisclaim`, which charges the helper itself.
   This works even from an SSH session. Registering the helper as its own
   LaunchAgent is the alternative.
3. **Ask once.** The helper's `access request` calls
   `AXIsProcessTrustedWithOptions` with the prompt option and opens the
   Accessibility pane. The helper then polls the permission, and exits once the
   permission arrives. `atuin setup` shows a final page that runs
   `access request` and waits for the helper to exit. The page offers "Open
   System Settings again" and "Turn off desktop capture", and esc grants later.
   `--yes`, `--only`, and runs without a terminal never wait, and their summary
   says that `atuin setup` grants the permission.

   The switch in the pane asks an administrator account for its own password. A
   standard account has to enter an administrator's name and password once.
   Capture itself doesn't need administrator rights. The pane is shared by
   every account on the Mac, and it shows one row per account's helper, with
   the name from step 1 and without a path. The system's alert, "would like to
   control this computer", stays open after the grant. The helper therefore
   asks the alert to quit once the grant arrives, and only when every alert
   window names the helper. The alert's process, `universalAccessAuthWarn`,
   shows up among the applications with content, and capture leaves the process
   out. The `apps` command asks the alert to quit, as setup does, and leaves
   the process out of the `apps` line. When setup sees the grant, it asks the
   daemon to start the helper at once. A grant made without setup waits for the
   daemon's next retry, which comes within 60 seconds.
4. **Per-application switches are rarely needed.** Chrome, Safari, and Electron
   build their trees when the helper asks, on every version from 13 to 27.
   Native applications and Chrome reject `AXManualAccessibility`, and VS Code
   accepts it. A VS Code that has just launched misses the switch on its first
   activation, so capture asks every application a second time, four seconds
   after the first request. See `quirks::WAKE_AGAIN`. VS Code's
   `"editor.accessibilitySupport": "on"` also removes the problem.
5. **Verify.** `atuin desktop-capture apps` and `atuin desktop-capture doctor`
   list the applications that own a window and every regular application, with
   the amount of readable content.
   `atuin desktop-capture doctor --outline <app>` prints the role tree of an
   application and marks what a read skips.

Beyond the grant, capture didn't need any package, service, or environment
variable on macOS 13, 14, 15, and 27. The same helper binary, built for
macOS 11 and later, ran on each version. A managed Mac is untested. Setup
cannot tell a denial by the organisation apart from a missing grant. [A managed
Mac](environments.md#a-managed-mac) describes what Apple documents about such a
Mac.

What remains, including shipping the helper and Developer ID signing, is in
[the roadmap](../roadmap.md#next-steps).
