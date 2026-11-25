# Desktop capture: the library

The modules of the `atuin-desktop-capture` library and how they fit together.
The library is what the helper is built from. Every future of the library is
`Send`. The helper drives it on a `current_thread` runtime. The library reports
what it skips and what fails through `tracing`, at debug level.

The interface to the desktop is three traits in `backend.rs`:

- `Platform` covers what belongs to the process: the permission (`access`,
  `request_access`, `open_access_settings`, `close_access_request`,
  `access_instructions`, `access_lifetime`, `take_responsibility`),
  `serve_main_thread`, and what setup tells the user about the session:
  `service_instructions`, `missing_app_notes`, and `session_variables`, the one
  list of the session variables that decide whether applications expose their
  content. The helper's `apps` and `doctor` and the CLI's login files and
  hiding checks all read that list.
- `Backend` connects to the desktop, wakes the applications that build their
  trees only on request (`wake_apps`), lists applications and windows (`apps`,
  `windows`, `active_window`, `active_window_of`, `hidden_documents`), tells
  what kind each window is (`window_class`), names the applications that
  capture leaves out (`is_left_out`), reads the content of a window as a tree
  of normalized `Node`s (`read`), and reports window events (`next`,
  `take_event_counts`).
- `Inspect` adds what setup and `doctor` need: `unready_apps`,
  `shows_only_prompts`, `app_info`, and `outline`, which lists the raw element
  tree for diagnosis.

`Desktop` is the backend of the current platform: AT-SPI on Linux, AX on macOS,
and elsewhere the backend of `platform/unsupported.rs`, which fails to connect
with `Error::Unsupported`. Callers therefore don't need conditional
compilation. All platform code sits under the `platform` module, one submodule
per platform. Everything else in the library, and the helper's and the CLI's
`run`, `doctor`, and `setup`, is written against the traits alone.

The platform-neutral modules never mention AT-SPI or AX:

- `capture/` holds `Capture`, which follows a backend's events and yields
  records: a segment started, a snapshot was taken, a segment ended, a dialog
  appeared, a notification arrived, and a background window was read. It holds
  the debouncing, the pacing of reads, polling, filtering, and scrubbing, and
  it doesn't store or print anything. Its submodules:
  - `active` follows the active window, `background` sweeps and re-reads the
    other windows, `dialogs` reads the dialogs that appear, and `notifications`
    hands out the notifications that the platform delivers apart from windows.
  - `debounce` and `pacing` decide when each read is due.
  - `scrubber` takes URL parameters, one-time codes, and credentials out of
    what capture hands out, through the `Redact` hook.
  - `config` holds the configuration, `context` what every part shares, such as
    the backend and the records that wait to be handed out, `record` the
    records themselves, and `title` the rules that tell the titles of one
    document from those of another.

  The method `Capture::stop` ends the open segment and the sweep, and reads or
  drops an edit that waits to settle, as its caller asks. `Capture::next` then
  drains the remaining records.
- `render` turns a tree into Markdown-like text. It puts the cells of a table's
  row on one line, set apart by `|`, and keeps inline formatting such as a web
  page's `<b>` and `<sup>` on the line of the text around it. `content` decides
  what counts as content in a tree, for the renderer and for the backends.
- `encoding` gives a tree a compact binary form that is the same for the same
  tree. The `serde` feature makes the tree serializable to other formats.
- `scrub` removes query strings, fragments, and user info from URLs and titles.
- `layout` holds the geometry of elements for both platforms: it orders the
  elements of a native window the way a person reads them, and tells whether
  two runs of text leave room for a blank between them.
- `readiness` holds what setup needs to know: which running applications don't
  expose anything until the user restarts them, which session file keeps
  applications from needing that restart, and whether a granted permission
  survives a new build.
- `private_windows` holds what each browser does to mark a private window, and
  reads the translations of those marks from the browser's own files. Its `pak`
  submodule reads Chromium's resource packs. See [Private browser
  windows](privacy.md#private-browser-windows).
- `outline` holds the types of `doctor --outline`.
- `link`, behind the `link` feature, holds the messages between the helper and
  the daemon. Its `process` submodule holds the rest of the contract with the
  helper's process: its exit codes and the variables that the daemon sets for
  it.
- `testing`, behind the `test-support` feature, is a scripted backend for the
  tests of code that drives a `Capture`.

Two more modules at the crate root don't call any platform API, but only the
Linux backend uses them, so they are compiled only on Linux:

- `flat` rebuilds a tree from a depth-first listing of its elements.
- `urls` tells the pages that a person can visit from the pages that a browser
  draws itself, such as `about:newtab`.

The Linux backend, under `platform/linux/`:

- `linux.rs` talks to AT-SPI over D-Bus with `zbus` and pipelines its calls.
  `bus.rs` holds the calls, `events.rs` the signals that the backend listens
  for and counts, `roles.rs` the roles by number, and `apps.rs` the kind and
  the toolkit of the application that each bus name belongs to.
- `read.rs` walks a window element by element, and `read/listing.rs` reads each
  document that the walk reaches from one listing of the document's elements.
  `read/budget.rs` holds the share of the node limit that the listings may
  still take. The crate-level `flat` module rebuilds a tree from such a
  listing, and `normalize.rs` brings AT-SPI's hypertext into the shape that
  every backend hands out. See [Reading a document from a
  listing](backends.md#reading-a-document-from-a-listing).
- `location.rs` finds the window and the document of the element that announced
  a change, so that capture follows only the changes of the window that it
  reads. `pauses.rs` holds the pauses of the applications that answered too
  slowly, for locations and for `apps.rs`.
- `notices.rs` monitors the session bus for the calls that post notifications.
- `outline.rs` lists the raw element tree of a window for `doctor --outline`.
- `procs.rs` reads `/proc`. Setup uses it to find browsers and Electron
  applications, and `private.rs` uses it to find the language packs beside a
  browser's executable.
- `private.rs` finds out whether a window belongs to a browser's private
  browsing mode. See [Private browser
  windows](privacy.md#private-browser-windows).
- `unlisted.rs` finds the elements that GTK 4 leaves out of the tree, such as
  libadwaita's spin rows, in the one listing of all elements that GTK 4 offers.
- `quirks.rs` holds every adjustment that exists for one toolkit. See [Where
  toolkit-specific code lives](backends.md#where-toolkit-specific-code-lives).

The macOS backend, under `platform/macos/`:

- `macos.rs` calls the AX, CoreFoundation, and CoreGraphics C APIs, which
  `ffi.rs` declares. `element.rs` holds retained AX elements and the
  CoreFoundation values of their attributes, and fetches several attributes in
  one call. `permission.rs` asks whether macOS trusts the process, reads how
  long the trust lasts, and closes the alert that asks for it.
- `watcher.rs` watches the desktop from the main thread: the frontmost
  application, the applications that own windows, and the notification banners.
  `observer.rs` holds what the watcher keeps on the run loop: observers of the
  AX API, subscriptions to notification centers, and the timer of the scans,
  with the backoff from applications that refuse an observer. `owners.rs`
  follows the applications that own windows, and `wakes.rs` notes when each
  application was asked to build its tree. `main_thread.rs` runs the main
  thread's run loop, which is the only one that receives `NSWorkspace`
  notifications.
- `desktop.rs` lists the applications and windows as the window server and the
  workspace list them. `dialogs.rs` holds the rules for which windows and
  groups are dialogs.
- `reader.rs` reads a window into a tree, and into the outline that
  `doctor --outline` prints. `banners.rs` reads the banners that Notification
  Center draws for every application. `private.rs` tells whether a window
  belongs to a browser's private browsing mode.
- `quirks.rs` holds the adjustments for single applications.
