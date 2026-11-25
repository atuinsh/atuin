# Desktop capture: privacy

What capture leaves out and scrubs, why, and how capture knows the private
windows of browsers.

## What capture leaves out

[The guide](../guide.md#what-capture-stores) tells users what capture leaves
out. In detail:

- Password fields are never read, and terminals are never read, wherever a
  terminal appears. A terminal inside another window, such as VS Code's
  terminal panel or a web terminal, shows as a `[terminal]` line without its
  title or its text. [Terminals inside windows](#terminals-inside-windows)
  describes how capture finds them.
- Menus, toolbars, buttons, and scroll bars are never read, because their
  labels are the same in every snapshot of an application and don't say
  anything about the work in the window. On Linux, a menu that a button opened
  is read while it shows, with its options.
- The browser address bar is never read, so the full URL stays out of a
  snapshot's text.
- Terminal emulators are left out entirely, with their window titles, on every
  platform. They are known by a list of application names. On Linux, a window
  that a terminal element fills by half or more counts as one too.
- The private windows of browsers are left out by default, with their titles
  and their background tabs. Firefox is known by the button that clears the
  private session, which only a private window has. Safari, Chrome, Chromium,
  Edge, and Brave are known by the title, which each browser translates.
  Capture reads the translations from the browser's own files, as [Private
  browser windows](#private-browser-windows) describes. A check that can't
  finish fails the read, so capture doesn't store anything of that read and
  tries again later.
- On macOS, capture doesn't read anything when the screen is locked. The lock
  screen shows the date and the full names of the Mac's users, so capture also
  leaves out its process, loginwindow. On Linux, capture doesn't check the
  lock.
- URLs keep only their scheme, host, and path. Titles lose URL parameters, and
  so do the names of the windows and documents in a snapshot.
- A URL that the text of a window, a dialog, or a notification mentions loses
  its credentials. It also loses its query or fragment when the query or
  fragment holds a parameter. For example,
  `https://app.example/#access_token=…` becomes `https://app.example/`, and
  `https://docs.example/guide#install` stays as it is. A word counts as a URL
  when it has a scheme, or when it starts with a host name, such as
  `example.org`, that a path, a query, or a fragment follows.
- Titles that a window shows only while it loads are never stored.
- A notification follows the application filter by the name of its sender, and
  terminals' notifications are left out. The filter also applies to the
  application that runs as the sender's process, because a sender can declare
  any name. On macOS, that is the running application that has the name that
  the banner shows. A notification that names its sender by an app id, such as
  `org.gnome.Fractal`, is left out when the filter excludes the name in the
  app's desktop entry or the name of a running application of that id. A
  Flatpak application's notifications arrive that way on GNOME. A notification
  without a sender is kept only when capture selects every application and
  doesn't exclude any.
- Application names are cleaned like titles, and cut to 64 characters, because
  a notification's sender chooses its name.
- Credentials are taken out of content, titles, URLs, and notifications with
  the patterns that Atuin uses for shell history, before anything is stored.
  The matching is best-effort.
- One-time codes are taken out of everything that capture stores:
  notifications, titles, and the text of windows and dialogs. A one-time code
  is a number of 4 to 8 digits next to a word such as "code", "PIN", or
  "verification". The matching is best-effort, so it can miss a code and can
  mask a number that isn't one, such as a ZIP code that a page calls a "code".

## Why capture leaves these out

The reasons behind the rules above come from testing:

- Password fields are masked or omitted on both platforms, including after text
  is typed.
- A token in a URL appeared in the address bar, the tab title, and the window
  title during page load. Field masking does not cover any of them, which is
  why URLs and titles are scrubbed and why titles that are shown only while a
  window loads are never stored.
- Applications expose whatever they display. An editor's output panel exposed a
  `PATH` dump, which is why content goes through the secret filtering that
  history uses.
- A terminal's title usually names the running command, so terminal emulators
  are excluded with their titles. Each platform knows them by a list of
  application names, and macOS also by bundle ids. On Linux, a window that a
  terminal element fills by half or more is treated the same, so the title of
  an unlisted terminal stays out too. An editor with a smaller terminal panel
  keeps its title and its text, without the panel's content.
- Notifications carry message previews and one-time codes. Exclusion by sender
  works on both platforms: the freedesktop bus names the sender, and on macOS
  the banner's description does.
- The windows of messaging apps and mail show the same one-time codes as the
  notifications, so capture takes the codes out of every title and tree. A
  one-time code is a run of 4 to 8 digits within 40 characters of a code word
  such as "code", "PIN", or "verification". Code words count only as whole
  words, and "code" doesn't count after a word such as "ZIP", "area", or
  "exit". A number that reads as a year or as a reference such as "#4711"
  counts only when a code word stands right before it, or when "is your" and a
  code word follow it. These rules were checked against the text of 15 web
  pages, the harness pages, the changelog and READMEs, and 25,000 lines of
  Rust, apart from the scrubber's own tests. The rules changed 4 lines, on
  pages about PINs, ZIP codes, and one-time passwords. Without the rules for
  years, references, and whole words, 70 lines lost numbers that weren't codes,
  mostly years and issue numbers.
- Secret filtering is best-effort and can be slow on large pages. It took about
  36 ms for a 170 KB page. A text that takes more than 2 seconds to filter is
  dropped whole, because keeping it unchecked is worse than losing it.

The feature will be compared to Microsoft Recall. It is text-only, local, and
opt-in, and the defaults have to hold up to that scrutiny. Capture selects
every application by default, and "Only the applications I list" is the choice
for users who trade convenience for control. Capture leaves the private windows
of browsers out by default.

## Terminals inside windows

A terminal holds commands, their output, and the secrets in both, so capture
doesn't read any terminal, wherever the terminal appears. A test on macOS on
2026-11-23 found that VS Code's terminal panel was stored, rows and output,
with `"editor.accessibilitySupport": "on"`, which the guide recommends. Capture
finds a terminal by what the element reports:

- **A native terminal** reports the AT-SPI role `terminal` on Linux. VTE and
  Konsole report it, including for Kate's terminal panel. macOS doesn't have
  such a role, so capture knows terminal applications there by their names and
  bundle ids only.
- **xterm.js** runs the terminals of VS Code, Cursor, JupyterLab, Hyper, and
  web terminals such as cloud shells. It draws on a canvas, and the browser
  reports its terminal element as a plain `section` or `AXGroup`. In screen
  reader mode, xterm.js puts a list with an item for each line below that
  element, a live region that repeats new output, and an input field that is
  named after the terminal's title. The element and its parts carry classes
  that start with `xterm`, and the live region has the class `live-region`.
  Chromium, Electron, and Firefox expose the classes in the AT-SPI attribute
  `class`, and Chrome, Electron, and Safari expose them in `AXDOMClassList`.
  The rule sits in each platform's `quirks` module. Safari, and Firefox in some
  reads, leave the terminal element out and show its parts side by side, so
  each part counts as a terminal too.

A terminal stays in the tree as an element with the role `Terminal`, without
its name, text, or children, and the text shows it as a `[terminal]` line,
which tells an agent where the terminal is. The parts of one terminal that
stand side by side show as one line. Capture's scrubber drops the name, the
text, and the children of a terminal regardless of what a reader handed over,
so the active window, dialogs, background windows, and the prompt hook all
follow the same rule. A dialog of a terminal panel, such as VS Code's question
before it ends a running process, is a window of its own and stays readable.

On Linux, a window that a terminal fills by half or more still counts as a
terminal window, which leaves out its title. The rule keeps the title of a
terminal emulator that isn't on the list of names, because such a title usually
names the running command.

On Linux, the check costs one `GetAttributes` call for each section, list, and
entry inside a document. The 12,000-element CI page in Chromium has 3,000
sections. The fastest of five reads of that page with
`atuin-desktop-capture doctor` took 594 ms without the check and 516 ms with
it, on a machine with a load of 21, so the cost stayed below the noise.

What each application exposed, on 2026-11-23:

| Application | What capture finds |
|---|---|
| VS Code 1.146 on Linux, `"on"` | the element `terminal xterm column-select focus`, with the rows, the live region, and an input field named "Terminal 1, probe Use Shift+Alt+F1 for terminal accessibility help" |
| VS Code 1.146 on macOS, `"on"` and `"auto"` | the terminal element. Before the rule, `"on"` stored the rows and the output, and `"auto"` stored the task's command line |
| xterm.js 6.1 in Chromium 156 and Chrome 157 | the element `terminal xterm xterm-dom-renderer-owner-1` |
| xterm.js 6.1 in Firefox ESR 153 | the same element. Firefox leaves out `xterm-accessibility` below it, and in some reads of a page that copies xterm.js's elements, the terminal element too |
| xterm.js 6.1 in Safari on macOS 27.2 | four or five parts side by side, without the terminal element |
| Kate's terminal panel, the Konsole part | the role `terminal`, whose text is the shell's |

The tab of a terminal panel stays readable, so its name, such as a task's name,
can show.

## Private browser windows

No platform marks a private window in a common way. A check on 2026-11-21 found
how each browser marks one. The check covered Firefox ESR 153, Chromium 156,
Edge 158, and Brave 1.100 on Linux, under X11 and sway. It also covered Firefox
160, Chrome, Safari, Edge 158, and Brave 1.100 on macOS 27.

- **Firefox** marks the window in a way that doesn't depend on the language.
  Only a private window has the "Clear private session" button, whose DOM id is
  `reset-pbm-toolbar-button`. AT-SPI exposes the id as the `id` attribute, and
  macOS exposes it as `AXDOMIdentifier`. A user can remove the button from the
  toolbar, so capture also matches the English title as a fallback. The title
  ends in "— Mozilla Firefox Private Browsing" on Linux and in "— Private
  Browsing" on macOS. On Linux, a window without a page is titled "Mozilla
  Firefox Private Browsing", and capture matches any brand before " Private
  Browsing", so that Firefox Nightly and the browsers that are built on Gecko
  match too. The translations of those titles are in firefox-l10n.
- **Safari** marks the window only in the title, which the format
  `%@, Private Browsing` builds. The window's identifier,
  `SafariWindow?IsSecure=false&UUID=…`, is the same for both kinds of window.
  Every language's translation of the format is in
  `/System/Library/PrivateFrameworks/Safari.framework/Resources/<lang>.lproj/Localizable.strings`
  under the key `"%@, Private Browsing"`. For example, the German format is
  `%@, „Privates Surfen“`. The key `"Private Browsing (window title)"` names a
  private window without a page. Capture reads the formats of every language
  directory, because Safari can run in another language than the helper.
- **Browsers built on Chromium** mark the window only in the accessible title.
  Chromium and Chrome append " (Incognito)", Edge appends " (InPrivate)", and
  Brave appends " (Private)". Chromium's X11 title lacks the suffix, and the
  window's class or app id is the same for both kinds of window. Every class of
  the Views tree also appears in a normal window. Each browser ships the
  translations in its locale files: on Linux in
  `<install dir>/locales/<lang>.pak`, and on macOS in
  `<Framework>.framework/Resources/<lang>.lproj/locale.pak`. A string's id
  changes from build to build, so capture finds the English format in the
  English file and reads the same id from the other files. Each English format
  is unique in its file. Edge's file also holds `$1 - [InPrivate]`, which the
  title doesn't use. The formats differ by language. For example, Brave has 56
  forms, such as `$1 (Privat)` and `$1（无痕）`, and Edge keeps "InPrivate" in all
  but five languages. The files for grammatical gender, such as
  `af_FEMININE.pak`, lack the string. Capture matches the formats of every
  language, because the browser's language can differ from the system's. A page
  title that ends in one of the formats is excluded as well, which errs toward
  privacy.

The platform backends mark a window as `WindowKind::Private`, and
`Config::captures` decides whether capture records a window of that kind. The
active window, the background sweep, and dialogs all ask it, so capture doesn't
store a segment, a title, or a background record for a private window.
`private_windows = "capture"` turns the exclusion off. Capture passes the
setting to every read in `Limits::private_windows`. With the exclusion on, the
read of a private window holds only the window's kind, so the backend stops
reading the window as soon as the check finds a mark. With the exclusion off,
the backend skips the check.

- **A failed check fails the read.** A window counts as private only when it
  carries a mark. A check that can't finish makes the read fail, so capture
  doesn't store anything from the read and tries again through its usual
  handling of failed reads. Nothing that came from a failure is cached. On
  Linux, an application that doesn't answer for its toolkit fails the read too,
  because only the toolkit tells whether the window needs the check. A read
  that records private windows doesn't need the check, so the read goes on
  without the toolkit. Firefox's button is cached only once it is found,
  because Firefox can expose a window before its toolbar. A main window of
  Firefox without any toolbar fails the check for the same reason. Translations
  that can't be read, or that can be read only in part, leave a known browser
  with the titles that were read for a minute, and then a read tries the
  translations again. Without any translation, those are the English titles.
- **Reading the packs is cheap.** The reader binary-searches each pack's index,
  so a browser's 80 or so packs cost a few small reads each, once for each
  browser process. A pack that can't be read adds the formats that were read
  before the failure. An English pack that doesn't hold any of the formats, as
  an Electron application's pack does, means that the application doesn't have
  private windows.
- **On Linux,** a browser is known by its toolkit, `Gecko` or `Chromium`. The
  check runs beside the walk of the window. Firefox's button is found with one
  `Collection.GetMatches` call on each toolbar of the window, and the toolbars
  are searched at the same time. The walk never enters toolbars, and a search
  of the whole window would search the page too. A background tab, which a read
  reaches as a document, takes the privacy of the window above it. An element
  whose role doesn't arrive is checked in the same way, which finds the element
  itself when it is a window. The packs are found beside the executable of the
  process that owns the application's connection to the bus, through
  `/proc/<pid>/root` first, so that a sandboxed browser's own view of its files
  counts. The packs are read on tokio's blocking pool, because the helper runs
  one thread for its events.
- **On macOS,** a Gecko browser is known by the `Contents/MacOS/XUL` library in
  its bundle, and a browser that is built on Chromium by a framework with
  `Resources/en.lproj/locale.pak`. Safari is known by its bundle id, or by the
  name of its bundle when the bundle id can't be looked up. An application that
  `NSRunningApplication` doesn't know fails the check. The check runs before
  the read. Firefox's button is found in the top three levels of the window,
  outside web areas, with one AX call for each element. A sheet or a dialog
  group takes the privacy of the window that its `AXWindow` names, and of its
  nearest ancestor that is a window when the named element isn't one. A browser
  that is built on Chromium shows a page's alert in a window of its own, whose
  attributes don't name the page's window. The page's window holds the same
  dialog element below its first group, so the alert's window takes the privacy
  of the other window of the process that holds the dialog. When no window
  holds the dialog, the check fails.

The harness's `private` scenario passed under sway, X11, KWin, and GNOME with
Firefox, Chromium, Edge, and Brave in German. The macOS runner's `private`
scenario passed twice on macOS 27 with Safari, Firefox, Chrome, Edge, and Brave
in German. [Known gaps](../known-gaps.md#private-windows) lists what remains
untested.
