# Desktop capture: what capture reads

What capture records from each window, the form in which the library hands it
out, and the rules that decide when capture reads a window. [Reading on each
platform](backends.md) describes how the Linux and macOS backends carry out the
reads.

## Segments and snapshots

A segment records the application, the window title, the document path or URL,
the start time, and the end time. Segments are nearly free to collect and join
with shell history on timestamps.

A snapshot holds the text of the active window. It is taken when a window is
activated, retitled, or loaded, and after its content stops changing. Snapshots
are deduplicated by content hash.

Outside documents, a snapshot holds what the window shows. Inside a document,
such as a web page, it holds the whole document of the active tab, including
the part that is scrolled out of view. Capturing only the visible part loses
too much: in the first real use, a README below the fold was missing, and the
error at the end of a CI log would be missing too. Whole documents also make
snapshots stable under scrolling, so deduplication works and scroll events are
not needed. Capture hands out every snapshot whole, and the daemon stores most
of them as deltas, as [Storing snapshots](storage.md) describes.

Options such as check boxes are rendered with their state, and spin buttons
with their value. In a settings dialog those are the content.

A window gets a stored segment only after it stays active long enough to be
snapshotted. Browsers show the URL as the title while a page loads, and this
rule keeps those titles out of storage. A retitle to another document, or a new
URL, starts a new segment. A title that only gains or loses a marker for
unsaved changes, such as TextEdit's "— Edited", keeps its segment. An edit that
waits to be read is snapshotted before capture follows another window, and when
the daemon pauses capture. On a pause, that read gets a budget of 500 ms, and
capture gives it up after a second, because the daemon gives a stopping helper
only a couple of seconds. A new configuration and a shutdown drop the edit,
because the daemon sends them when the user excludes an application or turns
capture off, and a read after that would store what the user no longer wants
captured. Dialogs, notifications, and background reads are the exception to the
rule: each is stored as soon as it is read.

A file-backed editor is stored as a pointer to its file. Atuin can read the
file when the content is needed, which avoids depending on each editor's
accessibility mode.

## Dialogs

A dialog or alert is read as soon as it appears, whether or not it becomes
active. The motivating case is an error that the user dismissed before reading
it: afterward, the message is nowhere on screen. Both platforms announce these
elements to screen readers, so anything that a screen reader announces, Atuin
can capture. Capture reads a window 100 ms after it appears, when the window is
a dialog or an alert, and hands it out as a record of its own, apart from the
segments of the active window. The daemon stores each dialog as a segment of
its own, which doesn't have a length. A dialog that reads blank, or whose read
fails, is tried again up to 3 times, 100 ms apart. A window whose application
doesn't answer what kind of window it is gets classified again in the same way.
An active window of an unknown kind may be a notification window, whose
one-time codes a segment would keep. Capture therefore doesn't follow such a
window. It activates the window again after 100 ms, up to 3 times, and follows
the window once the window's kind is known. A repeat of the same content within
a second is dropped. Reading a dialog costs 3 to 17 ms, so reading on
appearance is cheap.

On Linux, AT-SPI gives these windows the `dialog` and `alert` roles. A window
appears in one of four ways, and capture listens for all of them:

- `window:create`, from GTK 3.
- A child added to the root of an application, from GTK 4.
- A new application on the registry. A program that only shows one dialog, such
  as zenity, joins the bus with the dialog already open and doesn't send any
  window event, so capture lists the windows of every application that joins.
- Activation. Qt 6 doesn't announce a new window in any other way. An
  activation adds a record only for a window that doesn't have one yet, so a
  user who comes back to a dialog doesn't add anything.

libadwaita draws an `AdwAlertDialog` and a toast inside the parent window, as
an element with the `dialog` or `alert` role. No window appears. GTK 4
applications are therefore asked for the role of every element that they add,
and an added dialog or alert counts as a dialog that opened. Browsers add
children all the time, so only GTK 4 applications are asked. See
`quirks::shows_alerts_inside_windows`.

On macOS, the watcher observes `AXWindowCreated` and `AXSheetCreated` in every
application that owns a window, and an application that comes to own a window
counts as having appeared with its windows. A dialog has the subrole
`AXDialog`, `AXSystemDialog`, `AXApplicationDialog`, or
`AXApplicationAlertDialog`, or the role `AXSheet`. Every window of an agent
that shows only prompts, such as SecurityAgent, counts as a dialog too. A sheet
belongs to the segment of its window, and it is recorded as a dialog as well.
System agents show their prompts at the window layer of modal panels, 8, above
normal windows, so the watcher follows the owners of every layer from 0 to 8.
SecurityAgent's password prompt sits at layer 1000, so the prompt agents are
followed at every layer. The Gatekeeper prompt for a quarantined download
belongs to `CoreServicesUIAgent`, which shows one prompt at a time and creates
a new window for the next one when the user dismisses the first. A page's alert
is a dialog group inside the window in Safari and Firefox, which only a move of
the focus announces. The watcher finds it from `AXFocusedUIElementChanged`
through the enclosing dialog group. In Chrome, the alert is a window of its own
that holds only the alert, which `quirks::holds_page_alert` recognizes.

A dialog that is already open when capture starts is not recorded as a dialog
on either platform. The sweep at the start reads it as a background window, and
it gets a segment when it becomes active, like any other window.

## Notifications

A notification isn't a window of the application that sent it, so each platform
reads it where the platform delivers it. Capture records the sender, the title,
and the body as soon as the notification arrives. The daemon stores each
notification as a segment of the kind `notification`, with one snapshot, in the
same way as a dialog. The snapshot's tree is a window with one paragraph that
holds the body, and the segment's title is the notification's title. A
notification that the platform reports as a window becomes the same kind of
segment, with the tree that capture read from the window. Because a
notification is a segment, every command, MCP tool, deletion, eviction, and
search of the index handles notifications without a path of its own.

- On Linux, the session bus carries every notification to the notification
  daemon, regardless of which daemon draws it. Capture monitors the bus for
  `org.freedesktop.Notifications.Notify` and for
  `org.gtk.Notifications.AddNotification`, which GLib uses when GNOME Shell
  serves it. The call names the sender, and the body loses its markup. The bus
  also names the process behind the sender's connection, which the notice
  carries next to the name that the sender declared. Capture drops a
  notification when that process runs an application that the filter leaves
  out, as it does when the declared name is filtered. It finds the application
  by the process of each application's connection to the accessibility bus, and
  remembers the answer for up to 256 processes until an application joins. A
  lookup that fails is not remembered, and the notification is dropped when the
  `apps` setting lists applications. A notification that doesn't declare a name
  takes the name of the application that runs as its process. A Flatpak
  application's notifications come from its D-Bus proxy or from the
  notification portal, whose processes don't run any application. The portal
  passes a notification on to GNOME Shell as `AddNotification` with the Flatpak
  app id, such as `md.obsidian.Obsidian`, and a `Notify` can carry a
  `desktop-entry` hint. Capture therefore resolves a declared app id, and a
  declared name that reads as one, to the names of its desktop entry and to the
  running applications of the id. It looks for the entry in the XDG data
  directories and in Flatpak's export directories, and takes `Name` and its
  translations. A running application belongs to the id when the process of its
  accessibility connection runs in the id's Flatpak sandbox, or in a systemd
  unit named after the id, such as `app-gnome-org.gnome.Nautilus-1234.scope`.
  The notification is kept only when one of these names is in scope, none is
  excluded or a terminal's, and no running application of the id is a terminal.
  GNOME Shell's notification service passes each call on to the shell, so
  capture drops a notification that the owner of the name
  `org.freedesktop.Notifications` sends to another connection. A notification
  that the service sends to its own name is kept. A notification that an
  application sends twice is kept twice, on every desktop.

  The accessibility bus is not enough: dunst is not on it, and xfce4-notifyd
  draws a notification as a window with the `frame` role that names only the
  daemon. A window with the `notification` role counts as a notification only
  when the bus cannot be monitored. Capture reads such a window only as a
  notification, so the window doesn't get a segment as the active window or a
  read as a background window. Its notification segment keeps the tree that
  capture read from the window.
- On macOS, Notification Center draws every banner and alert as an element of
  one full-screen window at layer 23, above the layers that the watcher
  follows. It doesn't create a window for a banner. It announces the banner
  only with `AXLayoutChanged` on the banner element, which has the subrole
  `AXNotificationCenterBanner`, or `AXNotificationCenterAlert` for an alert
  that stays. The watcher observes that process for `AXLayoutChanged`, and
  reads each banner once by its identifier. The title, the subtitle, and the
  body are children of the banner. The name of the sender appears only in the
  banner's description.
  - On macOS 15 and later, the description reads "app, title, subtitle, body",
    and the name is what remains when the texts are taken off its end. On macOS
    13 and 14, the description holds only the name.
  - A notification without a title shows its sender's name in the place of the
    title, and a notice that the system sends about an application doesn't name
    a sender. The description of such a banner consists of its texts alone, or
    equals its title on macOS 13 and 14, so the banner doesn't tell its sender.
  - Capture scrubs the name of a notification's application as it scrubs the
    texts, and cuts the name to 64 characters, but a scrubber misses some
    secrets. The name therefore comes only from a description in one of these
    forms, and never equals a text. A banner that doesn't tell its sender
    becomes a notification without an application.
  - The name is in the language of the system, such as ターミナル for Terminal on a
    Japanese system. The notice names the process of the running application
    that has this name, so that capture checks the application's bundle id
    against the terminals and its name against the filter. A banner whose name
    doesn't belong to any running application, and that names an installed
    terminal in the language of the system, is skipped. Capture keeps a
    notification without an application only when the filter selects every
    application and doesn't exclude any, because the notification may come from
    an excluded application.
  - A banner without a description or a title is skipped. A banner whose
    description has another form is skipped too. For example, a child of a
    banner that slides in can still lack the text that the description holds.
    The next announcement of the banner reads it again.

## Background windows

Finding something across open windows needs the content of windows that the
user has not activated since capture started, and of background windows whose
content changes, such as a chat. Both are affordable, as long as each content
event is attributed to its window and its document. See [Performance of
background windows](performance.md#performance-of-background-windows).

- **The sweep.** Capture reads every window once when it starts, one window at
  a time with pauses between them, because one read of a large Chromium page
  stalls Chromium for 50 to 100 ms. After each read, the sweep rests for 20
  times as long as the read took, for 250 ms at least and 30 seconds at most,
  and dialogs and the active window go first. Capture hands out each window as
  a record of its own, apart from the segments of the active window, and the
  window keeps its window id when the user comes to it. The daemon stores the
  reads of a background window in a segment of their own, as [Storing
  snapshots](storage.md) describes. On Linux, the sweep waits until
  applications that join once a client connects have had time to join. In a
  private session, a sweep of 8 windows, with the 3,000-line page in Firefox
  and the 12,000-element page in Chromium, took 15 seconds.
- **Re-reads.** A background window is read again once its changes settle, at
  most every 10 seconds, or every 100 times as long as its last read took, up
  to 5 minutes. The read becomes a record only when the content differs from
  what capture last saw of the window, which is the last snapshot for a window
  that the user left. On macOS, every application that owns a window is
  observed for changes of content. A Firefox chat in a background window was
  recorded about every 10 seconds with its new messages, and the reads added
  about 0.5 seconds of Firefox CPU in 45 seconds.
- **Attribution.** Both backends attribute each content event to its window,
  and capture follows only the changes of the window that it reads. On Linux,
  the backend walks the parents of the element that announced the change up to
  the top-level window, and notes the nearest document on the way. The result
  is cached for every element on the path, so a new element costs one or two
  calls until it reaches a cached ancestor. On macOS, the element reports its
  window in one call.
- **Hidden documents.** On Linux, events from hidden documents are dropped.
  Firefox keeps a preloaded New Tab page in every window as a hidden document,
  and its rotating stories sent 96% of Firefox's content events. macOS doesn't
  need such a step: Safari, Chrome, and Firefox 160 on macOS 27.2 expose only
  the page of the active tab, and none of them sent an event in 60 seconds with
  a static page in front and a second tab behind it.

Firefox on Linux keeps every loaded background tab in the tree as a hidden
document with its whole text, and capture reads those tabs. The Linux backend
lists the hidden documents of a window that are pages a person can visit,
outside other documents, and capture sweeps them after their window and reads
them again when they change, as it does background windows. A change inside a
hidden page is reported as a change of the page's own handle, and one inside a
hidden page that the browser draws itself, such as `about:newtab`, is dropped.
The documents are found by walking the window's elements outside documents,
because a Collection query for documents took more than the one-second timeout
of a call in Firefox for a window with the 3,000-line page. Chromium exposes
only the active tab of each window, and so do all three browsers on macOS.

On macOS, background windows were read in TextEdit, Finder, and a test
application whose background window changed every 3 seconds:

- A minimized window is listed in `AXWindows`, readable, and announces its
  changes. An application whose windows are all minimized doesn't own a window
  on screen, so the backend also lists and observes every regular application,
  the ones with an icon in the Dock.
- A window in full screen has a Space of its own. `AXWindows` lists only the
  windows of the current Space, so the sweep does not see a window on another
  Space. Its changes still arrive, each with the window that holds the changed
  element, and capture reads it then. Listing the windows of other Spaces would
  take a private API.

Background reads capture content that the user may never have looked at, such
as a chat message that arrived in a hidden window. The opt-in has to say so.

## The edit stream

Text-change events carry the inserted text on most toolkits. This is
effectively a keylogger. It stays out of scope unless a named application needs
it.

## Snapshot format

The library hands out each snapshot as the element tree that the backend read.
Text is one view of the tree, made by `render` when a consumer asks, so no
later parsing or analysis is cut off by an early choice of format.

- **What a node holds.** The normalized role, the platform's own role name, the
  name, the element's own text, the heading level, the check mark, the
  visibility, the URL of a document, and the children. These are all values
  that a read fetches anyway.
- **One shape on every platform.** Text belongs to the element that shows it,
  and an element's content is its text or else its children in order. The Linux
  backend turns AT-SPI's hypertext, where a parent's text holds a placeholder
  for each embedded child, into that shape before it hands out the tree. A
  table's cells sit in row elements on every platform.
- **What costs extra stays out until it is asked for.** Positions inside
  documents, link targets, and the focused element each need more calls. They
  would be switches on a read.
- **What is never read.** Controls, hidden elements outside documents, and
  password fields are skipped at read time, and a tree does not bring them
  back.
- **Change detection.** The hash covers every field of every node, so two
  snapshots with the same hash are the same tree.
- **Depth.** A tree that nests deeper than `encoding::MAX_DEPTH`, 1,024
  elements, loses its deepest elements before it is hashed, and its read counts
  as truncated. Every tree that capture hands out therefore decodes.
- **Size.** A tree whose encoded form would take more than 32 MiB loses the end
  of its text and every element after that point in reading order, and its read
  counts as truncated. Every record therefore fits in a frame of the link,
  which holds 64 MiB at most. The cut comes after the secret filter, so it
  never keeps part of a credential that the filter replaces whole. A poll that
  finds the same tree as before skips both the filter and the cut.

The one place where the library renders a tree is a dialog without a title,
which gets the first line of its text as its title, up to 100 characters and
without Markdown markers.

The tree in `encoding`'s compact form against the rendered text, for the
windows that were measured:

| Window | Text | Tree | Text, zstd | Tree, zstd |
|---|---|---|---|---|
| Firefox, 3,000-line page | 170 KB | 343 KB | 8.9 KB | 9.8 KB |
| Chromium, 3,000-line page | 170 KB | 790 KB | 9.0 KB | 11.0 KB |
| Firefox, thread page | 5.9 KB | 31 KB | 0.5 KB | 1.1 KB |
| Firefox, page with forms and tables | 0.5 KB | 7.2 KB | 0.4 KB | 1.2 KB |
| VS Code, one small file | 0.6 KB | 25 KB | 0.4 KB | 2.0 KB |
| gedit, one small file | 0.1 KB | 2.6 KB | 0.1 KB | 0.5 KB |

Uncompressed, the tree is 2 to 40 times the text, and the ratio is worst for
small windows that are mostly layout. Compressed, it is 1.1 to 4.5 times the
text, and a few kilobytes at most. Storing the compressed tree alone, and
rendering text on demand, costs little more than storing text.


## Reading strategy

Both backends follow the same rules:

- **Read on events, and poll as a safety net.** Activation events carry the
  window. Content events schedule a debounced read. Events can be missing or
  misleading: a notebook tab switch in GTK doesn't send any of the content
  events. A window that has gone 30 seconds without a snapshot is therefore
  read anyway. That wait doubles, up to 5 minutes, when polls don't find
  anything new, and it resets when one does. A snapshot that finds the content
  of the segment's previous snapshot crosses the link as the content's hash
  alone, and the store adds one small record for it. A segment that has sent
  only hashes for the longest wait between polls, 5 minutes, sends its next
  content in full again. A store can fail to keep a content, and the hashes
  that point back to it would otherwise leave the segment without content.
  Capture also keeps the hash of the tree as the read found it, so a read of
  the same tree skips the secret filter. Background reads skip the secret
  filter in the same way when a window shows the same tree as before. A poll
  that doesn't find anything new therefore costs one read, one hash of the
  tree, and that record. The `poll_interval` setting changes the 30 seconds or
  turns polling off, and `read_budget` bounds how long one read may take. The
  daemon sends both to the helper with the rest of its configuration.
- **Skip what is hidden outside documents.** The reader stops descending at
  hidden elements, such as the pages of unselected tabs. Inside a document it
  reads everything.
- **Treat appearing and disappearing elements as a change.** Switching a
  notebook tab in GTK sends only "showing" state changes.
- **Pace content snapshots by their cost.** After a snapshot, content changes
  can trigger the next one only once the window has rested for 20 times as long
  as its last read took, and for 1 second at least and 30 seconds at most.
  Reading takes a twentieth of the time at worst, and a window that is cheap to
  read stays responsive. A gap that doubles when snapshots come back unchanged
  doesn't work: tooltips and other noise push it to its maximum, and the next
  real change waits that long.
- **Settle quickly after a rearrangement.** A window that was activated,
  retitled, loaded, or rearranged is read after 400 ms of quiet, because those
  changes arrive as one burst. Edited text waits 1.5 seconds, because typing
  sends an event for every key. A steady stream of changes postpones a snapshot
  by 5 seconds at most.
- **Stay with the window that opened a popup.** The list of a combo box or a
  menu is a window of its own that takes the active state. Capture ignores it,
  so the window that opened it keeps its segment and no empty segment appears.
  A hand-over to a popup waits 200 ms.
- **Skip controls and secrets.** Menus, toolbars, buttons, password fields, and
  terminal widgets are not read. This also keeps the browser address bar out of
  snapshots. On Linux, a menu that a button opened is read while it shows, with
  its options and their check marks. Chromium's own windows, such as the
  address bar's suggestions, are skipped.
- **Bound every read.** Each call has a timeout of a second, and each read has
  an element limit and a time budget. A hung application must not block capture
  from the others. A read that runs out of time is kept and tried again after
  the window rests, three times in a row at most, because an application that
  is busy answers slowly for a while.
- **Retry a failed read.** A read that fails doesn't store anything, and it is
  tried again three times in a row at most. The active window is read again
  once its change settles again, a dialog 100 ms later, and a background window
  once the window rests. A background window whose reads keep failing waits
  until its content changes again. Capture keeps what it last handed out for
  the window, so a read that finds the same content then doesn't hand it out
  again.
- **Rest a window that isn't captured.** When a read finds that the active
  window is a private window, a terminal, or a browser's own interface, capture
  stops following the window. A focus change inside the window reads the window
  again only once the window has rested as long as it would after a snapshot,
  because every focus change would otherwise read the whole window. A focus
  change during the rest is kept, and capture looks at the window when the rest
  ends, if the window is still active. A window's kind can change, such as when
  the user hides the terminal panel of an editor.
- **Resolve every activation to a top-level window.** Firefox announces
  activation on an inner object that doesn't hold any of the page.
- **Retry an empty snapshot.** An application can expose its tree before the
  text in it.
