# Desktop capture: known gaps

What capture is known to miss, to read wrongly, or to leave on disk, on each
platform.

## Deleted and evicted content

- The daemon deletes the old files of the store, and doesn't overwrite them.
  Until the file system reuses their blocks, a tool that reads the raw disk can
  still find the deleted content. Backups and snapshots of the data directory
  also keep what they copied.
- A segment that the disk budget evicts can stay in fjall's journal and in the
  search index's files until the next rewrite of the store, which also scrubs
  the index. Evictions rewrite the store once they add up to a quarter of the
  store, at most every two hours. The store counts the evictions across
  restarts.
- When a step after a deletion fails, the deleted content can stay in the files
  of the store or of the index until a later attempt succeeds. The steps are
  the rewrite of the store, the removal of the store's old files, the removal
  from the index, and the scrub of the index. The daemon logs each failure, but
  `atuin desktop-capture delete` still reports the deletion. The store and the
  index note each owed step with the deletion, so the daemon tries the steps
  again every 15 minutes and at its next start. A daemon that stops in the
  middle of a deletion also finishes the steps at its next start.
- A rewrite needs as much free disk space as the store takes, and the rewrite
  fails without that space.
- A rewrite copies the whole store, and reads and writes of the store wait
  while it runs. A release build rewrites a store of 10,000 dialogs in about
  0.2 seconds, and a store of 70 MB in about 0.3 seconds, so a store of several
  gigabytes takes several seconds. The scrub of the index merges the whole
  index. A release build scrubs an index of 15,000 contents, 27 MB on disk, in
  about 0.2 seconds.

## Private windows

- On Linux, dialogs of a private browser window are untested, and capture may
  record them. Capture reads a dialog on its own, but the marks of a private
  window are on the window. On macOS, a sheet or a dialog takes the privacy of
  its window, as [Private browser
  windows](architecture/privacy.md#private-browser-windows) describes.
- A notification names only its application, so a notification from a private
  window is captured like any other.
- Firefox moves the buttons of a narrow window into a menu that is closed most
  of the time, and a user can remove the button that clears the private
  session. Capture then knows the private window only by its English title.
- Vivaldi, Opera, and Arc aren't known, and neither are Brave's private windows
  with Tor, Chrome's guest windows, or Firefox set never to remember history.
- When capture leaves out private windows, a main window of Firefox without
  any toolbar can't be read, because the check takes it for a window whose
  toolbar is still to come. A window-modal dialog that hides the toolbars, such
  as the onboarding of a new Firefox profile on macOS, has the same effect
  while it shows. On macOS, an application that `NSRunningApplication` doesn't
  know can't be read either, because the check can't tell whether it is a
  browser.
- Chromium-based browsers in a Flatpak sandbox are untested. If capture can't
  read their translations, it knows their private windows only by their English
  titles.

## Linux

- Qt 5 applications on Wayland don't expose anything, because Qt 5's Wayland
  platform plugin never starts Qt's AT-SPI bridge. Such an application runs
  readably under Xwayland with `QT_QPA_PLATFORM=xcb`. It can still register an
  empty root with the toolkit "gtk" when it loads GTK, so `doctor` lists it as
  a GTK application without windows.
- Qt 5 applications on X11 register with the accessibility registry only by
  chance, because of a race in Qt 5.15. See [Qt 5 on
  X11](architecture/environments.md#qt-5-on-x11).
- On at-spi2-core 2.60 and earlier, which every LTS distribution ships, capture
  doesn't read Qt applications or Firefox until
  `org.gnome.desktop.interface toolkit-accessibility` is on. Setup turns the
  key on, except where GSettings doesn't have a dconf backend to keep it.
- GTK 4 doesn't expose the rows of a `GtkTreeView` over AT-SPI, so capture
  reads such a list as empty. One example is the list of applications in the
  "Open With" dialog of Nautilus 48.
- GTK 4 leaves the spin rows of libadwaita out of the tree. Capture finds a
  spin row only after the row took the focus, so a spin row below the first
  field of a dialog is missing until the user tabs or clicks into it. See
  `quirks::leaves_roleless_elements_out`.
- LibreOffice Calc doesn't report any children for a sheet, so capture reads
  the window around a sheet and none of its cells.
- Dolphin shows errors in a message bar inside its window rather than in a
  dialog. The bar is in the window's snapshots, but the prompt hook doesn't
  give it a dialog line.
- Capture skips what lies inside a button, apart from a menu. GTK 4's System
  Monitor puts its CPU figures inside an expander button, so its Resources page
  reads as its tab names.
- Linux records a notification that a client sent even if the notification
  daemon rejected it. When the client gave neither a name nor a desktop entry,
  the notification takes the name of the application that runs as the client's
  process. Without such an application, capture keeps the notification only
  when it selects every application and excludes none.
- A minimized Kate window reported an empty name to every client in a private
  session, so the sweep stored it without a title.
- A background tab of Firefox that changed shortly before the user switched to
  it is read once more as a background window after the switch. A background
  record names the tab's own handle, and not the window that holds the tab.
- Capture doesn't check whether the screen is locked on Linux.
- In the harness's X11 session, a zenity alert that stayed for 300 ms was
  missed in 2 runs of 21 while a chat in Firefox kept changing in front.

## macOS

- A process's dialog that opens and closes within a second, without the process
  launching or activating, is lost, because macOS lists the owners of windows
  once a second. After an application activates without a window, the list is
  read every 100 ms for a second. UserNotificationCenter's alerts that showed
  for 450 ms were all recorded, and ones that showed for 300 ms were all lost.
- macOS shows only the newest banner of notifications that arrive within about
  a second of each other. Capture records only the banners that show.
- Capture takes the sender of a notification from the text of the banner, and
  that parsing was tested only in English. A banner of a notification without a
  title doesn't tell its sender, and neither does a notice that the system
  sends about an application.
- loginwindow becomes the active application a moment before macOS marks the
  session as locked, so capture can learn of a lock up to a second late.
  Capture always leaves loginwindow out, and a read fails once the session is
  marked as locked, so the lock screen is never stored. A window that changed
  in that moment is read only after the unlock, when capture reads every window
  again.
- A running helper keeps its Accessibility permission after
  `tccutil reset Accessibility`. On 15.7.7, a helper captured for 10 minutes
  after the reset, until it restarted, and a new process was refused at once.
  Turning the switch off in System Settings stops capture within 2 seconds. AX
  reports the permission from a cache in the process, so the helper can't
  notice a reset sooner without starting a process to ask.
- Turning capture off and on again leaves a stale entry in the Accessibility
  pane. Turning capture off deletes the signing certificate, and turning
  capture on signs the helper with a new one. The old entry then shows as on
  but doesn't apply to the new helper, so setup keeps waiting for the
  permission. Removing the old entry with the minus button fixes it on 14.8.7
  and 15.7.7: a new entry appears within a minute, and capture starts once it
  is turned on.
- After a switch to another user and back, a tester once saw every read come
  back empty until a reboot. That user was still in the setup assistant, and
  its session was ended with `sudo launchctl bootout gui/<uid>`. On 15.7.7 in a
  VM, two normal switches with a logout from the menu and two such bootouts all
  left the reads intact. The cause is unknown.
- The sweep doesn't see windows on other Spaces, including windows in full
  screen. Such a window is read once it changes.
- Chrome reports heading levels and the state of a page's check boxes only to a
  client that sets `AXEnhancedUserInterface`, which capture leaves off.
- Numbers exposes the cells of a sheet only while `AXEnhancedUserInterface` is
  set, so capture sets it on Numbers alone. Pages and Keynote were not checked.

## Every platform

- Capture finds a terminal inside a window by the AT-SPI role `terminal` or by
  the classes of xterm.js. A terminal that has neither is read like any other
  element. JetBrains IDEs, GNOME Builder, and web terminals other than
  xterm.js, such as hterm, were not checked. Zed doesn't expose anything to
  read. VS Code's accessible view of a terminal, which opens with Alt+F2 and
  shows the terminal's text in a view of its own, was not checked.
- On Linux, WebKitGTK applications such as GNOME Web were not checked for
  xterm.js's classes.
- The tab of a terminal panel stays readable. Its name can be a task's name or
  the running command.
- VS Code exposes the text of its editor only with
  `"editor.accessibilitySupport": "on"`, followed by a restart, because it
  doesn't detect a screen reader on Linux or macOS. Without the setting,
  capture reads VS Code's panels and tabs and none of the code. With the
  setting, an editor exposes about 500 lines around the cursor. On macOS, the
  first visit to a VS Code that has just launched doesn't store any editor text
  without the setting.
- Firefox's PDF viewer exposes the text only of the pages that it has rendered,
  and it runs each page's lines together. Chromium's viewer exposes the whole
  document, one line per line.
- Chromium leaves out the space between an inline-block element and the text
  after it, so the line numbers of some CI logs run into their lines, as in
  `1818error: …`.
- Chromium leaves out a blank between words when the page puts comments around
  it, as ChatGPT does. Capture puts most such blanks back, but a single lost
  blank in a long paragraph of plain runs stays lost.
- Search splits text into words at blanks and punctuation, so in Chinese,
  Japanese, or Korean text a query matches only a whole run of characters
  between punctuation. For example, `デプロイ` does not find `本番環境へのデプロイは金曜日の18時`.
