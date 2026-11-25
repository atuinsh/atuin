# Desktop capture: performance

What reads cost the applications and capture, how the design keeps that cost
down, and where capture was tested.

## Cost of a read

Read cost, measured in release builds before documents were read from listings:

| Window | Linux | macOS |
|---|---|---|
| Text editor | 3 to 20 ms | 6 to 8 ms |
| VS Code | not measured | 14 to 20 ms |
| Dialog | 9 to 15 ms | 1.4 to 6.4 ms |
| Browser, 3,000-line page, visible part | 110 to 200 ms | about 150 ms |
| Browser, GitHub repository page, whole document | 90 to 270 ms | not measured |
| Browser, 3,000-line page, whole document | 800 to 900 ms | not measured |

The whole-document figures on Linux are for a walk. Reading a document from a
listing halves them. See [Reading a document from a
listing](backends.md#reading-a-document-from-a-listing).

Ways to read the same 12,000-element page:

| Approach | Time |
|---|---|
| Linux, Python, one call at a time | 5,900 ms |
| Linux, Rust, one call at a time | 2,700 ms |
| Linux, Rust, pipelined, whole tree with text | 500 ms |
| Linux, `Cache.GetItems`, whole tree without text, only when the cache is complete | 127 ms |
| Linux, Collection query for visible text | 78 ms |
| macOS, batched walk of the whole tree | 300 ms |
| macOS, whole page as one string through text markers | 33 to 57 ms |

- **The application pays most of the cost.** An application answers every call
  on its own UI thread, so a read spends more CPU inside the application than
  inside the capture process.

  | Read, Linux | Elements | Wall time | Application CPU | Capture CPU |
  |---|---|---|---|---|
  | gedit, small file | 44 | 20 ms | below 1 ms | 9 ms |
  | Firefox, GitHub repository page | 1,245 | 100 ms | 75 ms | 52 ms |
  | Firefox, 3,000-line page | 3,037 | 237 ms | 176 ms | 124 ms |
  | Chromium, 3,000-line page | 12,000 | 810 ms | 800 ms | 405 ms |

  The capture CPU includes process startup, because each read was a separate
  `doctor` run.
- **A read is many small calls, so the application stays responsive between
  them.** A second client that called the application every 2 ms waited 0.2 to
  0.6 ms at the median and at most 1.7 ms at the 99th percentile during reads.
  Stalls were rare. gedit had none. Firefox had none above 16 ms in the settled
  runs, and a few of 60 to 85 ms in runs made soon after it started. Chromium
  stalled for 50 to 100 ms in about half of its reads of the largest page. A
  stall of that size drops a few frames. It is noticeable while scrolling or
  typing, and it is not a freeze. The probe measures how long the UI thread
  kept another caller waiting. It does not measure dropped frames directly.
- **Polling makes the cost continuous.** One read of the largest page every 30
  seconds costs Chromium about 2.7% of a core, and the back-off brings that to
  about 0.3% once the page is quiet. Typical pages cost a tenth of that.
- **Idle capture costs almost nothing.** Both buses are silent when nothing
  is happening. On macOS, an idle capture used 0.01 s of CPU in 30 s.
- **Some applications are noisy.** An idle Firefox window sends tree-change
  events from the preloaded New Tab page, a hidden document in every window.
  The Linux backend finds the document of each content event and drops the
  event when the document is hidden. Events that show or hide elements always
  count, because a tab switch in Firefox announces itself before the document
  that it shows reports the showing state. An idle Firefox window with the
  3,000-line page was re-read 18 times in 60 seconds when those events
  counted, and not at all once they were dropped.
- **Content events name their window.** Attributed only to its application, a
  change in a background window of the active application would re-read the
  active window. A chat in a background window, with the active window of the
  same browser idle for 40 seconds, caused 7 unchanged reads in Chromium and 6
  to 7 in Firefox that way, and none once events named their window. On macOS,
  a test application with a text area that changed every 3 seconds in its
  background window caused 9 unchanged reads in 30 seconds, and none once
  events named their window.
- VS Code sends two title changes per keystroke. The minimum interval between
  content snapshots bounds this, and the cost per application belongs in the
  measurements from real desktops.
- A browser page is the expensive case.
- **The feature costs little to build.** It adds about 3% to a clean release
  build on Linux, 246 s against 239.5 s, as the best of two runs each, mostly
  for the eight zbus crates. On macOS, it adds only `block2` and `dispatch2`,
  because `arboard` already brings in objc2. `atuin` then links
  ApplicationServices too, but loads the same 690 images at launch either way,
  because AppKit already loads ApplicationServices.

## Performance of background windows

Measured on Linux in a private X session, with a release build of two probe
programs. The desktop had a chat page that posted a message every 6 seconds in
one Firefox window, the 3,000-line page in a second Firefox window with a
second tab behind it, the 12,000-element CI page in Chromium, a minimized Kate
window, and gedit as the active window. The probes swept every window, then
re-read a background window after 2 seconds of quiet, 5 seconds after its first
event at most, and 10 seconds after its last read at the earliest.

| Measurement | Result |
|---|---|
| Sweep of 8 windows | 650 to 700 ms. The Chromium page took 450 ms, the 3,000-line page 170 to 220 ms, and every other window 1 to 10 ms. |
| Re-reads on any content event of the application | Firefox sent 390 events in 45 s. Each burst re-read both of its background windows, and half of the reads found nothing new. |
| Re-reads on the events of the window, with hidden documents dropped | 498 events in 60 s led to 5 reads, which all found new content. They took 68 ms in total. |
| Attribution of the 498 events | 636 calls and 56 ms in total, and 69 cached elements. |
| Where Firefox's events came from | 478 from the hidden New Tab pages, 20 from the chat. In the first 30 s after the start, 254 of 264. |
| CPU of the capture process | 0.4 s in 60 s, including the sweep. |

The sweep found text in the minimized Kate window and in the background Firefox
window, and the re-reads found a chat message 21 seconds after the experiment
started, in a window that was never activated. A background page that keeps
changing, such as a live CI log, needs a longer rest than the active window, or
a 12,000-element page read every 10 seconds costs Chromium several percent of a
core. Capture therefore rests a background window for 100 times as long as its
last read took, where the active window rests for 20 times as long.

## Where capture was tested

- A private Xvfb session with xfwm4: gedit, Kate, Firefox, Chromium, VS Code,
  and system-config-printer.
- A GNOME 48 session on Wayland: gedit, gnome-text-editor, Nautilus, GNOME
  Settings, Rhythmbox, Evince, Kate, Dolphin, Wireshark, Firefox, Chromium, VS
  Code, Obsidian from Flatpak, zenity, and polkit's prompt.
- The harness's sessions, which [Wayland](backends.md#wayland) lists, and real
  logins in the Linux distributions that [What each environment
  provides](environments.md#linux) lists.
- macOS 26.6: TextEdit, Safari, Chrome, and VS Code.
- macOS 27.2: TextEdit and its save sheet, Finder and its alerts, System
  Settings, Preview, Safari, Chrome, Firefox, Edge, Brave, VS Code, Activity
  Monitor, Notes, Numbers, SecurityAgent's password prompt,
  UserNotificationCenter's alerts and permission prompts, and notifications
  from `osascript`.
- macOS 13.7.4, 14.8.7, and 15.7.7 in virtual machines: TextEdit, Safari, VS
  Code, and setup on fresh accounts, as [What each environment
  provides](environments.md#macos) describes.
