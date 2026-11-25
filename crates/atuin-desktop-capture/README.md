# atuin-desktop-capture

Desktop capture records what the applications on the desktop show, through the
accessibility APIs of Linux (AT-SPI) and macOS (AX). The Atuin daemon stores
what capture saw on the user's machine, and serves it to commands, to MCP
tools, and to Claude Code's prompts. An agent then knows what the user was just
doing, such as the error page that the user read before asking for a fix.

Desktop capture is an early preview, and it doesn't come with any compatibility
guarantees. Its commands, its settings, and the data that it stores can change
from one build to the next. A later build may require deleting what capture
stored, or running `atuin setup` again.

It has two parts:

- `crates/atuin-desktop-capture` is the library that every desktop read goes
  through. The library doesn't store anything or have a user interface. It
  isn't published.
- `atuin-desktop-capture` is the helper that the daemon runs. The `atuin`
  package builds it as a second binary with the `desktop-capture` feature. The
  helper does every read of the desktop, so that only the helper needs the
  platform's permission.

It records five things: which application and window was active, the text of
the active window, dialogs and alerts as soon as they appear, notifications,
and the text of background windows.

## Platforms

- **Linux**: X11 and Wayland sessions, with GTK, Qt 6, Firefox, Chromium, and
  Electron applications. [Known gaps](docs/known-gaps.md#linux) lists the
  exceptions, such as Qt 5 on Wayland.
- **macOS**: tested on versions 13 to 27. The helper needs the Accessibility
  permission.
- Atuin offers capture only on Linux and macOS.

## Documentation

- [docs/guide.md](docs/guide.md): a quick start, then installing capture,
  turning it on, seeing and deleting what it stores, and the reference of its
  commands, MCP tools, prompt hook, and settings.
- [docs/known-gaps.md](docs/known-gaps.md): what capture is known to miss on
  each platform.
- [docs/architecture/](docs/architecture/README.md): how capture works, and why
  it is built this way.
- [docs/development.md](docs/development.md): building capture, using the
  library, running the test harness, and rehearsing the demo.
- [docs/roadmap.md](docs/roadmap.md): what capture is for, and what comes next.
