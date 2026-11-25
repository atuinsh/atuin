# Desktop capture: roadmap

What desktop capture is for, what has to happen before its first release, and
what is still unknown. [The guide](guide.md) describes how to use desktop
capture, and [the architecture](architecture/README.md) describes how it works.

## Goals

- Record which application, window, and document the user worked in, and when.
- Record the text of the active window when it changes meaningfully.
- Record dialogs and notifications as they appear, including ones that close
  within a moment.
- Keep the content of every open window searchable, including windows in the
  background.
- Make that context available to search, to command inspection, and to agents
  through MCP.
- Keep the user in explicit control of which applications are captured.

Out of scope: screenshots, OCR, input logging, applications that draw to an
opaque canvas, and terminal emulators. Atuin already captures terminals through
the shell.

## What we would do with it

The value comes from joining the desktop with what Atuin already has: shell
history, command output, and agent threads. Each use below needs both halves.
The best uses also don't ask anything new of the user, such as a habit to learn
or a command to remember.

### The goal: agents that know what the user was just doing

The user asks an agent a short question in everyday words, and Atuin supplies
what the user was reading or discussing elsewhere on the desktop. Three demos
show this:

1. **A bare reference.** The user reads WG14 proposal N3254 at
   `open-std.org/jtc1/sc22/wg14/www/docs/n3254.pdf`, or a Stack Overflow answer
   about it, and then asks Claude Code "does my compiler have any issues
   related to n3254?" Alone, "n3254" is ambiguous. Recent activity shows that
   it is a proposal for the C language. Nothing in the prompt hints that
   outside context matters, so this demo is the hardest of the three. The URL
   alone resolves the reference, so segments carry it even without the text of
   the PDF.
2. **An error on screen.** A CI log fails in the browser, and the user tells
   the agent "fix this". The agent reads the whole log, including the error at
   its end, and matches the job to the local repository. A variant: the user is
   scrolled to one section of a design document and asks the agent to
   "implement the section I'm reading", which uses the visible extent.
3. **A conversation elsewhere.** The user works through a design in claude.ai,
   ChatGPT, or the Claude desktop app, then asks Claude Code to "implement what
   I worked out in my chat earlier". Atuin has agent threads only from tools
   that report to it. Desktop capture gives it the raw text of the other chats,
   without structure. The demo shows how large the gap is between no context
   and some context, even when that context is unpolished.

An agent does not reliably call an MCP tool on a prompt that looks
self-contained, such as the N3254 question. Two ways to get the context to the
agent:

- **A prompt hook.** Atuin's Claude Code hook attaches a few lines of recent
  activity: titles and URLs from the last several minutes. The full text stays
  behind MCP tools. This reaches the model on prompts that do not look like
  they need context, but it sends titles to the model with every prompt, so the
  summary has to be short, cover a short window, and be opt-in.
- **MCP tool descriptions alone.** This is cheaper and sends less, but is less
  reliable for prompts like the N3254 question.

Atuin uses both. The [prompt hook](guide.md#the-prompt-hook-for-claude-code)
adds the summary to each prompt on `UserPromptSubmit`, and the MCP tools serve
the full text. The open question is how often the summary changes the answer,
which step 1 of [Testing in real use](#testing-in-real-use) measures. Claude
Code's `SessionStart` event, which runs once when a session starts or resumes,
could also link the new thread to what was on screen when the thread began.

### Beyond the browser

If every demo ran in a browser, the feature would look like a browser
extension. Accessibility reads any application that supports screen readers, so
the demos must span toolkits: GTK, Qt, Cocoa, and Electron, with the browser as
one case among several.

1. **The vanished error (GTK, Cocoa).** A copy in Nautilus or Finder fails, and
   the user closes the dialog before reading it. The user asks "what was that
   error?", and the agent reads the dialog, then checks the cause from the
   terminal, such as a full disk. A variant on macOS: Gatekeeper blocks a
   binary that the user downloaded with `curl`, and the agent ties the dialog
   to the command that fetched the binary. Another variant: a notification
   about a failed backup or update disappears before the user reads it. Error
   popups that close or disappear and take their message with them are a common
   frustration, so this demo needs no explanation.
2. **"Is everything back to how it was?" (GNOME, KDE, macOS settings).** The
   user changes settings to fix a problem, fails, and wants to restore the
   settings, but does not remember what they were. The baseline is the first
   snapshot of each settings page, because the user sees a page before changing
   it. Atuin lists every setting that differs from the baseline, and the agent
   points out risky ones that the user forgot to undo, such as an enabled SSH
   server, a disabled firewall, or file sharing. Changes made in the terminal,
   such as `ufw disable`, are in history, so the answer covers both. A page
   that was never opened doesn't have a baseline, but its settings were also
   not changed through the GUI.
3. **A failed connection from another device (Qt).** A phone or an embedded
   device fails to connect to a development server that runs in the terminal.
   The server's captured output shows a handshake error from the device's
   address, and Wireshark's packet details pane shows that the device's
   ClientHello offers only TLS 1.0. The request came from another device, so
   nothing in shell history can rerun it, and the agent has to read Wireshark
   to answer. With a request from `curl`, the agent could have rerun the
   command under `tshark`, and the demo would show nothing new.
4. **Finding a window (any toolkit).** Windows pile up on most desktops, and
   finding one means going through every application. The user asks for "the
   document that mentioned the migration deadline", and Atuin searches every
   open and recently closed window. The answer names the application and the
   window, and shows the content that matched. This needs the background
   windows from [What capture reads](architecture/reading.md).

   Bringing the window to the front is optional, and the demo works without it.
   Everything that Atuin does with the desktop so far is read-only, so acting
   on a window deserves its own design before Atuin does it. An agent may not
   need Atuin for it: if the MCP tools return the application, the window
   title, and the process, an agent can try to raise the window with the
   platform's own tools, such as `osascript` or `wmctrl`.
5. **A build error in Xcode (Cocoa).** "Fix the build error." The agent reads
   the issue navigator or the build log. This is the native counterpart to the
   CI log demo.
6. **Numbers in a spreadsheet (LibreOffice Calc).** "Load the numbers I'm
   looking at into the script." Calc doesn't report any children for a sheet,
   so capture doesn't read any of its cells today. Reading the visible cells
   would need a walk through the table interface.

A set that covers the range: the vanished error, the settings, the failed
connection, finding a window, and the N3254 question. Those run in GTK or
Cocoa, Qt, any toolkit, and the browser, and four of the five join the desktop
with terminal history.

### Other uses

In order of expected value:

1. **Errors already solved.** When the captured output of a failing command
   matches an earlier error, Atuin shows what came after the earlier error: the
   pages that the user read and the next command that succeeded. It is passive
   and built from data that Atuin already has.
2. **Where a command came from.** Inspecting an old command shows the document
   that the command was pasted from, with the surrounding text that explains
   why it was run. Bracketed paste in `pty-proxy` marks pasted commands, and
   the snapshots from the minutes before the paste hold the source.
3. **Runbooks from mixed sessions.** A session that moves between a web
   console, a dashboard, and the terminal becomes a written procedure. Snapshot
   diffs show settings that changed, because check boxes are rendered with
   their state and spin buttons with their value. An agent can translate the
   console steps into equivalent CLI commands.
4. **Commands seen elsewhere as suggestions.** A command from a chat message or
   a document appears as an inline suggestion when the user types its prefix in
   the shell. This depends on Atuin having inline suggestions, because a search
   over seen commands asks the user to search for something that they never
   ran, and fails silently when capture missed it. Seen commands need these
   limits:
   - Only recently seen commands qualify, and only after a typed prefix, so a
     document with hundreds of commands does not drown out the user's history.
   - A command from history wins over a seen command.
   - A seen command is text that a web page controls. Text can be in the
     accessibility tree without being visible, so only text that was in the
     visible extent qualifies, every suggestion names its source, and accepting
     one takes a deliberate key.
5. **Reconstructing a day.** Answer "what did I work on yesterday" across the
   terminal, browser, editor, and agent threads.
6. **Search over everything seen.** The most powerful use and the riskiest.

Every demo also needs the privacy controls shown alongside it: the indicator,
the allowlist, a way to forget recent capture, and a view of exactly what is
stored.

## Next steps

The steps fall into three groups. The first group is the work that has to land
before the first release. The second group is testing in real use, which looks
for bugs and gaps. The third group can follow the release. The early preview
runs from the feature branch, as [the guide](guide.md) describes.

### Before the first release

1. **Check the Linux setup in real logins.** The desktop capture choice of
   `atuin setup` prepares every session that the user can log in to, as [Setup
   on Linux](architecture/setup.md#setup-on-linux) describes. Logins after
   setup confirmed this on Fedora 45 with GNOME, on Ubuntu 26.04 and 24.04 with
   GNOME, and on Kubuntu 26.04 with KDE Plasma on Wayland and X11. They also
   confirmed it on Debian 13 with GNOME and with XFCE from LightDM, on openSUSE
   Tumbleweed with sway from greetd, and on Arch with sway, Hyprland, and i3
   from a console. These parts remain:
   - **Real logins.** Log in after setup on Arch with GDM and XFCE, on Fedora
     with GDM and KDE Plasma on Wayland, and on a console with fish as the
     login shell that starts sway. Check that capture runs before any terminal
     opens.
   - **Capture before the desktop exists.** A shell file can run before the
     graphical session starts. For example, GNOME runs the login shell before
     it starts its own services. A daemon that starts then runs the helper at
     once, because nothing in capture waits for a display. The helper's first
     call can start the accessibility bus outside the session, and without
     `DISPLAY` the bus launcher never sets the property of the X11 root window
     that Qt 5 reads. Find out whether this happens, and if it does, make the
     helper wait for a graphical session before it connects.
   - **Compositors that GDM starts directly.** sway, Hyprland, or i3 that GDM
     starts don't read any of the files. Setup should print the lines for their
     own configuration.
   - **The report of `toolkit-accessibility`.** On at-spi2-core 2.61 and later,
     the first listener writes `true` to the key anyway, so the key doesn't
     change anything there. Setup could tell the two behaviours apart by
     registering a listener and reading `IsEnabled`, and say which one the
     machine has.
   - **GSettings without dconf.** Where the key doesn't persist, Qt and Firefox
     stay off the bus on at-spi2-core 2.60 and earlier. Capture could write the
     flag at each connection instead.
   - **Qt 5 in `doctor`.** `doctor` could report Qt 5 applications as best
     effort on X11 and unreadable on Wayland, as [What setup
     handles](architecture/setup.md#what-setup-handles) describes them.
2. **Ship the helper.** Releases leave the helper out, because
   `desktop-capture` isn't a default feature of `atuin`, and the daemon doesn't
   run capture when it doesn't find a helper next to its executable. The helper
   is a second binary of the `atuin` package, as
   [Processes](architecture/overview.md#processes) describes. The release still
   needs these parts:
   - Decide whether `desktop-capture` is on by default. With the feature on by
     default, a plain `cargo install atuin` would install the helper, and dist
     would put the helper into release archives.
   - Publish `atuin-desktop-capture`, which `atuin` then depends on.
   - The release workflow, the install script, Homebrew, Nix, and
     `cargo install` each need to put the helper where the daemon looks, on
     Linux and on macOS.
   - Tell distribution packagers about the second binary. Nix and
     `cargo install --path` pick it up, but many packages copy
     `target/release/atuin` by name.

   The helper stays a separate binary on both platforms. On macOS, the separate
   binary keeps the Accessibility grant on code that only reads the desktop. A
   grant to `atuin` itself would let any program that runs `atuin` with
   responsibility disclaimed control the desktop through the processes that
   `atuin` spawns, such as scripts and shells. A separate binary is also the
   only file that has to be signed. Linux doesn't have such a permission, but
   one shape on both platforms is simpler than a subcommand on Linux and a
   helper on macOS.

   A helper built from source on macOS has only an ad-hoc signature. For such a
   helper, the certificate that setup creates keeps the grant across updates.
3. **Sync captured records between machines.** The team has asked for it, and
   it should ship with the first release. Deletions have to sync too.
4. **Decide the privacy defaults.** Capture selects every application by
   default, and excludes [private browser
   windows](architecture/privacy.md#private-browser-windows). Use of the
   preview may change these defaults. Decide which applications are excluded by
   default, and decide on a default retention period too. Today the store is
   bounded only by its disk budget, which is 10% of the disk by default. Decide
   also whether the prompt hook needs an opt-in of its own. Today turning
   capture on registers the hook when Atuin's Claude Code hooks are installed,
   and setup says which context leaves the machine with each prompt.
5. **Show that capture is on.** The feature will be compared to Microsoft
   Recall, so the user needs a visible indicator when capture is running. The
   preview runs without one. Find out first what each platform allows. For
   example, macOS has menu bar items, and stock GNOME shows tray icons only
   with an extension. The indicator may need a process that stays in the
   session, such as the helper.
6. **Write the user documentation.** The docs site has guides for output
   capture but not for desktop capture. [The guide](guide.md) in this crate is
   the starting point: it covers what is captured, the privacy controls, setup
   on each platform, and how to use `doctor` when an application reads as
   empty. The guide moves to the docs site with the first release. The site
   deploys from `main`, so it shouldn't describe desktop capture before users
   can install a release with it. The guides on the site set Atuin up through
   `atuin setup`, so the guide for desktop capture should start there too.

### Testing in real use

Capture has run on GNOME on Wayland, xfwm4, sway, KWin, a real i3 desktop, and
two Macs, against the applications that [Known gaps](known-gaps.md) lists, and
in both demos. What remains:

1. Keep capture on during daily work on both platforms, and fix what goes
   wrong. Measure what only days of use show: how fast the store grows against
   `max_disk_usage`, the helper's CPU and memory, and whether the prompt hook's
   context helps an agent or distracts it.
2. Finish the check of what the environment needs. [What each environment
   provides](architecture/environments.md) holds the results so far. They cover
   Fedora 45, Arch, Ubuntu 26.04 and 24.04, Debian 13, Kubuntu 26.04, and
   openSUSE Tumbleweed, both in containers and in real logins. On macOS, they
   cover versions 13, 14, 15, and 27. These checks remain:
   - Run KDE Plasma on at-spi2-core 2.62, for example on Fedora 45 KDE.
   - Find out whether Firefox and Chromium in a Flatpak sandbox follow
     `toolkit-accessibility`, which both read when they cannot ask the bus.
   - Find out how Hyprland 0.57's Lua configuration sets an environment
     variable, so that setup can print the right line.
   - Test a Mac whose management profile controls the Accessibility pane, when
     an MDM is available.
3. Check notification banners on macOS 13 and 14 with real notifications. The
   rule that reads the sender from a banner whose description holds only the
   sender's name comes from the banners' AX trees.
4. Try more Electron applications on Linux, such as Slack and Discord. The hook
   that builds the tree is in Chromium itself, so they should behave as VS Code
   and Obsidian do.
5. Check the private windows that capture doesn't know yet: Vivaldi's, Opera's,
   and Arc's, Brave's private windows with Tor, Chrome's guest windows, Firefox
   160 on Linux, and Firefox set never to remember history, which makes every
   window private. Check the dialogs that a private window shows on Linux too.
   Find Firefox's button that clears the private session also in the overflow
   menu, where a narrow window moves the button.
6. Check whether chat applications such as claude.ai keep a long conversation
   in the tree or only the part near the viewport. PDFs in Firefox's and
   Chromium's viewers were checked. See [Known
   gaps](known-gaps.md#every-platform).

### After the first release

1. Decide on Developer ID signing for macOS releases. Until then, the
   certificate that setup creates keeps the grant. Once a release ships a
   helper with a Developer ID signature, the daemon should run that helper as
   it is. Today the daemon signs a copy of any helper with the certificate,
   which would replace the Developer ID signature.
2. Wrap the signed copy of the helper in an app bundle, which gives the helper
   a name and an icon in System Settings. Check that the helper's relaunch and
   the grant both work from inside the bundle.
3. Track what was on screen at capture time. A stored tree cannot tell which
   part of the window was on screen. The helper would have to mark the visible
   part of each tree when it reads the tree.
4. Add the prompt hook for Codex. Codex sends `UserPromptSubmit` too, but
   neither the output that Codex reads context from nor the trust hash of a
   hook entry without a matcher is documented. Mistral Vibe doesn't offer any
   hooks, so Vibe reaches desktop context only through the MCP tools that setup
   registers for it.
5. Consider recording the active window of applications that don't expose an
   accessibility tree on Linux. It is undecided whether this is worth the work.
   One example is an imgui application that draws with GLFW or SDL. Such an
   application never registers on the accessibility bus, so capture doesn't see
   an activation, and the time spent in it shows up as a gap without an
   application or a title. macOS doesn't need a change, because AppKit exposes
   the window and its title for every `NSWindow`, so capture should record a
   segment there without text. No such application was tested on macOS. Linux
   would need a second source of the active window. X11 has
   `_NET_ACTIVE_WINDOW`. Wayland doesn't have a portable protocol, so each
   compositor would need its own code, such as a GNOME Shell extension like the
   harness's or a KWin script. Such a segment would carry only the
   application's name and the window's title, and the terminal filter would
   have to apply to it as well.
6. Consider which other applications `atuin desktop-capture apps` should leave
   out of its suggestion. The suggestion names every application with a known
   name and readable text, in the order of the scan, apart from the names in
   the helper's `LEFT_OUT`. That list holds the macOS permission alert and the
   system prompt agents, such as `kwalletd6`, whose first-run wizard had stayed
   open for days in a GNOME session in daily use. Other groups may belong too,
   such as desktop shells and panels, notification daemons, input methods, and
   settings applets. A filter would change only the suggestion. `apps = "all"`
   would still capture these applications, because a prompt that the user sees
   can be useful context.
7. Decide how a deletion erases content on disk, for history, output capture,
   and desktop capture together, because all three can hold the same secret. A
   deletion in any of them removes the content from what Atuin reads and
   searches, and the bytes stay on disk until the storage engine rewrites them.
   Desktop capture compacts its store and merges its index after a deletion.
   Output capture and history only remove their rows. The journals of fjall and
   SQLite can hold copies too. No compaction or checkpoint reaches the copies
   that the disk and the file system keep, such as blocks that an SSD remapped,
   snapshots, and backups. Crypto-shredding would reach them: each record is
   encrypted with its own key, and a deletion destroys the key. It only works
   if the keys live somewhere that can really be erased, such as the system
   keychain. The record store already encrypts each record with its own key, so
   the idea fits with syncing captured records, in step 3 of [Before the first
   release](#before-the-first-release). Destroying a key would also erase the
   copy on the sync server.
8. Consider reading a whole browser page on macOS through text markers, if the
   cost of reads matters in practice. The 12,000-element page took 33 to 57 ms
   that way, against 300 ms for a walk, as
   [Performance](architecture/performance.md) shows.
9. Consider reporting a stop apart from the state of capture. The daemon's
   status reply has `Stopped` as a state and a `stopped` field as well, so
   `status` has to avoid saying the stop twice.

## Possible directions

Desktop capture is the first part of Atuin that works with applications outside
the terminal. That could make a graphical interface worth considering at some
point, for example for setup. Desktop capture doesn't depend on one, and the
first release ships with Atuin as a terminal tool. Some parts of capture would
suit a graphical interface if Atuin ever had one. Two examples are the
indicator from step 5 of [Before the first release](#before-the-first-release),
and the setup steps that only work at the Mac's screen.

## Open questions

- Should Atuin raise a window that a search found? No use depends on it, and
  capture is read-only otherwise. If it should, macOS has `AXRaise`. AT-SPI
  doesn't have a general way to raise a window, and Wayland compositors
  restrict which clients may.
- What does turning accessibility on cost in memory and CPU for heavy Chromium
  and Electron applications? One 3,000-line page in Chrome didn't show any
  difference beyond normal variation.
- Does a Developer ID signature keep the grant across updates the way the
  self-signed certificate does?
- Can a dialog be read while it is still filling in? Capture hands out the
  first read of a dialog that is not blank, and reads it no further, so a
  dialog that adds its content a moment later would be stored incomplete. Every
  dialog read so far was complete: GTK 3, GTK 4, Qt 6, and zenity build a
  dialog before they announce it. Electron and web dialogs, which can fill in
  asynchronously, are untested. If it happens, capture can read the dialog
  again after another settle, hand out a further record of the same window
  while the content keeps changing, and stop when two reads match, when the
  dialog closes, or after the longest wait. A dialog that holds a loading
  document would get the same treatment.
- Is Windows UI Automation worth a backend? It has a similar model. Under the
  current policy it would be best-effort.

