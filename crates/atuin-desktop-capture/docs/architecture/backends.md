# Desktop capture: reading on each platform

How the Linux and macOS backends read windows, the toolkit differences that
each one handles, and where the code for a single toolkit lives. Both backends
follow the rules of [Reading strategy](reading.md#reading-strategy).

## Linux: AT-SPI over D-Bus

- The backend uses `zbus` and pipelines calls, with up to 32 in flight.
  Pipelining cuts the cost of a call from about 72 µs to about 10 µs.
- Events must be registered with the registry in its own spelling, such as
  `Window:Activate:` and `Object:StateChanged:Active`. A malformed registration
  is ignored without an error.
- A change of a check mark counts as a change of content. The backend listens
  for `Object:StateChanged:Checked` and drops repeats of the same value.
- GTK3 reports the children of a grid in reverse visual order. Outside
  documents, the reader fetches the position of every element and orders
  siblings by rows and then columns.
- GTK3 sends only a focus event for its first window, and no activation event.
  Focus moving to another application therefore counts as an activation.
- GTK4 announces a window before its application object answers. The name
  lookup retries.
- xfwm4 keeps an unnamed window of the `window` role active and showing at all
  times. A window that looks like a popup therefore counts as active only when
  no other window is.
- Firefox returns an empty string for a text range that ends at -1. The reader
  asks for the character count and requests the exact range.
- Chromium names the document URL attribute `URI`. Firefox names it `DocURL`.
- Chromium and Electron build the tree of a page only after a client asks an
  element for its relations. The backend notes when it first asks each
  application for relations, and reports a window without a document as loading
  for the second after that, regardless of the toolkit. A native window, such
  as GTK's, therefore waits up to a second for its first snapshot after the
  application's first request.
- Toolkits disagree on inline text. Chromium folds inline text children into
  the text of their parent. Firefox gives every child a placeholder character
  in the parent's text, including inline wrappers such as `<code>`. The Linux
  backend tells the two apart by counting placeholders, and resolves both into
  runs of text between the children.
- Chromium leaves out a blank between words when the page puts comments around
  it, as ChatGPT's answers do. The document reader puts such a blank back where
  the positions of the characters on each side of the seam leave room for it.
  It does so only in an element whose text holds fewer than half as many words
  as the element has children, so a single lost blank in a long paragraph of
  plain runs stays lost.
- Chromium also leaves out the space between an inline-block element and the
  text after it, so the line numbers of some CI logs run into their lines.
  Document reads fetch positions only where Chromium drops the runs of blanks
  between words, so this space stays out.
- Firefox includes the bullet in the text of a list item.
- GNOME Shell's own surfaces, such as the overview and the top bar, aren't
  windows to capture. A dialog that the shell adds to its stage, such as
  polkit's prompt, counts as a window that opened. See [Wayland](#wayland).
- `Cache.GetItems` returns the roles, names, and states of many elements in one
  call, without their text. It is not used to read windows, because what it
  returns depends on the toolkit and on timing:
  - GTK3 lists every showing element, and the entries are correct.
  - GTK 4 lists only the elements that a client has reached already. GTK 4 adds
    an element when a client gets a reference to it, such as through a parent,
    a child list, or a relation, and when the element's own state changes. A
    freshly started GNOME Calculator listed 2 elements with its preferences
    open. After one walk, the cache held exactly the walked elements. Their
    roles, names, states, parents, and child counts all matched the walk. In
    GNOME Text Editor, 22 of 97 showing elements entered the cache only through
    the walk. A call costs about 2 ms in GNOME Calculator, and about 16 ms in a
    Nautilus that had listed 2,000 elements.
  - Qt doesn't return anything, and Firefox returns two entries.
  - Chromium lists what existed when the first client registered for events,
    plus what the page added through scripts afterwards. A page that loads
    while a client is registered is missing: after a navigation, the cache held
    239 of 12,244 elements.
  - An application without a registered client doesn't have a cache interface
    at all.

  The GTK 4 reader makes one `GetItems` call per read, for the elements that
  GTK 4 leaves out of the tree. See `quirks::leaves_roleless_elements_out`. No
  client can reach such an element, so the cache holds one only after the
  element took the focus. The spin row of GNOME Calculator is the first field
  of its preferences, and it was in the cache whenever the user opened them,
  with or without a click into it. Opened through the application's D-Bus
  action in a window that was not active, the row didn't take the focus and
  stayed out of the cache. Relations of every element of the walk did not bring
  it in, and neither did a change of its value from `gsettings`.
- Peer-to-peer connections to applications are unavailable in at-spi2 2.62.

The Orca screen reader's source shows how a mature client handles the same
problems, and capture follows it in these ways:

- **It asks the window for its document.** A browser window has an `EMBEDS`
  relation that points at the document of the active tab. Firefox and Chromium
  both expose it, and GTK and Qt windows don't have one. One call finds the
  active document and its URL without a walk. Chromium points the relation at
  whichever document had the focus last, which can be a side panel. The Linux
  backend therefore reports the first URL that a person can visit, and asks the
  embedded document before the others.
- **It waits while a document is busy.** A loading document carries the `BUSY`
  state, and Orca ignores its events until `object:state-changed:busy` clears.
  On Linux, a read reports a showing document that is still loading, and the
  snapshot waits, for 5 seconds at most. Firefox and Chromium both set the
  state while a response is arriving.
- **It checks more than the active state of a window.** A window counts as
  active only when it is also showing and is not iconified. The Linux backend
  applies the same rule.
- **It marks a hung application.** After a call fails, Orca skips the
  application for a second and drops its events. Capture still tries every
  element of a read, within the read's budget. To attribute events, the Linux
  backend pauses an application for 5 seconds after it fails to answer, and for
  a second after it takes more than 50 ms to place one element. A second slow
  placement right after a pause counts as no answer. The application's events
  don't carry a window when the application is paused. Finding out what kind of
  application sent an event may take 250 ms, and an application that takes
  longer counts as an ordinary one, without calls, for 2 seconds.
- **It distrusts cached state.** Orca clears the client-side cache in about
  thirty places before it relies on a state. That matches the `Cache.GetItems`
  findings above.
- **It thins event floods.** Orca drops an event when the same application sent
  the same kind less than 100 ms earlier, and skips an event that a newer one
  supersedes. The debounce in capture has the same effect.
- **It reads text the same way.** Orca fetches the character count and then
  that range.
- **It uses Collection queries** to find elements by role inside a document.
  The Linux backend lists a whole document with one query. See [Reading a
  document from a listing](#reading-a-document-from-a-listing).

## Wayland

AT-SPI is a D-Bus protocol, so capture reads the same trees under Wayland. What
differs is what the compositor tells each toolkit, and how a test drives the
session. The [harness](../../harness/README.md) runs the same scenarios in
private headless sessions of X11 with xfwm4, KWin 6.3, KWin with Xwayland,
mutter 48, GNOME Shell 48, and sway 1.10, with GTK 3 (gedit), GTK 4 (zenity),
Qt 6 (Kate), Firefox, Chromium, and VS Code as native Wayland clients. All of
its checks pass in every kind of session, apart from notifications in plain
mutter, which doesn't have a notification service. Chromium and VS Code are
read with `--ozone-platform=wayland`, and Chromium on X11 under Xwayland too.

- **Qt activates the window that loses the focus.** When another application
  takes the focus, Qt on Wayland sends an activation and a deactivation of its
  own window in the same moment. Followed blindly, the activation ends the
  current segment, and a window that has just opened never gets its first
  snapshot. The Linux backend holds each activation for 50 ms, and a
  deactivation of the same window during that time cancels it.
- **Positions are relative to the window.** A Wayland client does not know
  where its window is on screen, so GTK and Qt report positions inside the
  window. The reader compares positions only within one window, to order
  siblings, to measure the area of a terminal, and to clip the visible extent,
  so relative positions serve as well.
- **GTK 3 is active only with the keyboard focus.** A headless compositor
  without a keyboard never gives GTK 3 the keyboard focus, so gedit never
  reports its window active. Qt follows the compositor's activated state
  instead.
  Every real session has a keyboard, and the harness adds a virtual one.
- **GNOME Shell is an application on the bus.** Its stage, "Main stage", holds
  the top bar, notifications, and system prompts such as the keyring prompt,
  and it has the active state when one of those holds the keyboard. Followed
  like a window, the stage would record a prompt that the user sees, and also a
  clock that changes every minute, with each line repeated because its labels
  have the same name and text. Capture leaves the shell's own surfaces out, and
  records the dialogs that the shell adds to its stage, such as polkit's
  prompt. See `quirks::is_desktop_shell`.
- **A read can run out of time when the desktop is busy.** In sway, the
  12,000-element page in Chromium took 1.7 seconds to read while VS Code
  started, beyond the budget of 1.5 seconds, and the snapshot held only
  Chromium's bars. Both readers report a read that stopped at its time budget
  apart from one that stopped at its element limit, so that capture can try the
  first kind again. See [Reading strategy](reading.md#reading-strategy).
- The Firefox and Chromium snaps of Ubuntu 26.04 reach the accessibility bus
  through the `desktop-legacy` interface, which snapd connects automatically,
  and receive the session's variables. They behave like unconfined builds.
  Flatpak is covered in [Chromium and Electron don't need a launch
  switch](setup.md#chromium-and-electron-dont-need-a-launch-switch).

## Reading a document from a listing

One Collection query returns every element of a document in depth-first order:
5 ms for 3,000 elements in Firefox, and 86 ms for 12,000 in Chromium. With all
elements known up front, the read needs three rounds of calls, each sent for
all elements together:

1. The role and the child count of every element. The child counts turn the
   listing back into a tree.
2. The text of every readable element, except plain runs of text.
3. The text of the plain runs that the parent does not already repeat. Chromium
   repeats them all, so three quarters of its elements don't need a text call.
   Firefox doesn't repeat any.

The window itself is still walked, and every document that the walk reaches is
read from a listing. A browser window therefore keeps its own bars, and docked
developer tools and side panels are read the same way as the page. Reading only
the document that the window points at loses the developer tools.

A document is walked like the rest when it is larger than what is left of the
node limit, when it nests deeper than the depth limit, when the child counts
and the listing disagree, when the read runs out of its time budget, or when
only the visible part is wanted. The listings of one level of the walk run at
once, so each one reserves its elements from what is left of the node limit,
and the query asks for no more elements than that.

A read stops at half of the depth that the snapshot decoder accepts, because
normalizing a table adds a level. An element whose role does not arrive is read
as an empty container, because it could be a password field. A read in which a
role call timed out reports that it ran out of time, so that capture reads the
window again. The walk, the outline, and the search for documents each visit an
element once, so that an application that lists an element below itself cannot
make them loop. Where Chromium drops the blanks between words, the listing also
fetches the positions of characters to put the blanks back.

Inferring the child counts from the placeholders in each text would save the
first round, but it does not work: Chromium gives a placeholder to an inline
element such as `<code>` but none to a plain run, both have the same role, and
elements without text can have children. `normalize.rs` tells the two apart by
comparing the placeholders against the children.

Measured on the 3,000-line test page, median of 6 reads, against the walk:

| | Walk | Listing |
|---|---|---|
| Chromium, 12,000 elements | 800 ms | 410 ms |
| Firefox, 3,000 elements | 240 ms | 145 ms |

The rendered text is identical on that page, on a page with nested lists,
tables, forms, inline code, an iframe, and a password field, and on that page
with the developer tools open.

Further cases, each compared against the walk:

| Case | Result |
|---|---|
| Two tabs, Firefox and Chromium | Only the active tab is read. The other tab's title and content are absent. |
| Chromium bookmarks manager | Read: folders and bookmark names. |
| Chromium bookmarks side panel | Read next to the page. The window still reports the page's URL. |
| Firefox Library window | Read as its own segment: the folder tree. |
| Firefox bookmarks sidebar | Read next to the page. |
| VS Code | Read: the file tree, the editor text, and the chat panel. |

## macOS: the AX API

- The backend calls the AX, CoreFoundation, and CoreGraphics C APIs directly.
- `AXUIElementCopyMultipleAttributeValues` fetches all attributes of an element
  in one call, at 20 to 30 µs per element. Each AX call has a timeout of a
  second.
- Observers are per process. The watcher observes every application that owns a
  window, and every regular application. An application that launches when
  capture is running can refuse every registration with
  `kAXErrorCannotComplete` until it is ready. The watcher drops such an
  observer and tries again after a backoff that starts at a second and grows to
  30 seconds, because each attempt on a hung application blocks the main thread
  for the AX call's timeout. Only `kAXErrorCannotComplete`,
  `kAXErrorInvalidUIElement`, and `kAXErrorAPIDisabled` drop an application's
  observer, because they mean that the whole application cannot answer. Another
  error skips only the notification that it concerns. The backoff covers the
  frontmost application too. When the frontmost application is backed off, the
  watcher doesn't ask it for its focused window and doesn't wake it again. A
  terminal gets an observer only for new windows and sheets, because capture
  never reads a terminal's content.
- The watcher starts paused, and stays paused until capture runs. A paused
  watcher doesn't read banners, titles, or content, and doesn't wake any
  application. A pause drops every observer, and a resume looks at the desktop
  again from the start.
- macOS doesn't have a "showing" state. An element is out of view when its
  frame lies outside the window. Chrome clamps scrolled-out elements to the
  edge of the page and doesn't give them any area, so inside a document a frame
  without area is out of view too.
- Chrome builds its tree after a client reads `AXRole` of the application
  element. Electron builds its tree after a client sets
  `AXManualAccessibility`. Safari builds its page about a second after a client
  first asks, and doesn't announce anything when the page arrives. The backend
  notes when it first asks each application to build its tree, and reports a
  window without a document as loading for the 2 seconds after that. A native
  window without a document, such as TextEdit's, therefore waits up to 2
  seconds for its first snapshot after the application's first request.
- The backend reports a loading document through `AXLoaded`.
- The window's `AXDocument` attribute gives the file path for editors and the
  URL for Chrome.
- The system-wide `AXFocusedApplication` attribute fails with
  `kAXErrorCannotComplete` in a process that does not initialize AppKit.
  `NSWorkspace` notifications work there, but only when the main thread is
  running its run loop. Observers that a background thread registered didn't
  receive anything, whether that thread ran its own run loop or AppKit was
  initialized on the main thread, and `frontmostApplication` went stale. The
  watcher therefore runs on the main thread, and a program hands that thread
  over through `Platform::serve_main_thread`. The helper owns its process, so
  giving up the main thread costs it nothing.
- The watcher follows activation from `NSWorkspace`. The frontmost application
  is the one that macOS reports, so an application that is frontmost without a
  window ends the segment of the application behind it. No notification
  announces a process that shows its first window without launching or
  activating, such as an alert from a process without other windows, so the
  watcher also lists the owners of windows on every activation, launch, and
  quit, and once a second. When the frontmost application doesn't have a window
  yet, the watcher lists them every 100 ms, up to 10 times in a row.
  `NSWorkspace` cannot replace that list: it leaves out processes that own
  windows, such as `WebThumbnailExtension`, and its launch notification leaves
  out agents.
- The watcher reads `CGSSessionScreenIsLocked` at the same moments as the
  owners of windows, and the lock and unlock notifications trigger a check of
  their own. It reports each change as `Event::ScreenChanged`, before the
  events of the same look, and capture doesn't read anything until the unlock.
  A locked session doesn't have a frontmost application or any owners of
  windows. loginwindow activates a moment before the session is marked as
  locked, so the backend leaves loginwindow out, and `read` fails when the
  session is locked. The backend reads the flag for each read, so that a read
  that capture scheduled just before the lock can't find the lock screen.
- The reader fetches the attributes of an element in one call and walks the
  window. A switch, a pop-up button, or a one-line field that names a label of
  its own, as System Settings and save panels do, reads as the label with the
  value, such as `[x] Show indicators for open applications` or
  `Dock position on screen: Bottom`. Tables and outlines read one row per line,
  after a row of the columns' headings. An icon with a title, as in Finder's
  icon view, reads as its title. Runs of text in a page that do not touch get a
  space between them, which the frames of the runs tell.

## Where toolkit-specific code lives

The backends follow rules that hold for every application, and a rule that
exists for one toolkit sits in the `quirks` module of its platform, with the
toolkit, the difference, and how it was observed. The platform-neutral modules
don't know about any application, with one exception: `private_windows` holds
what each browser does to mark a private window, because both platforms share
that knowledge. Each platform's `quirks` gives the formats of the titles that
differ between the platforms. `src/platform/linux/quirks.rs` and
`src/platform/macos/quirks.rs` list every adjustment. Some adjustments that one
toolkit made necessary are safe for every application, and the backend applies
them to every application. On Linux, such an adjustment has a **Scope** line
that tells which applications it reaches, and why that is safe.
