# Desktop capture: architecture

How desktop capture works, and why it is built this way. Start with the
overview.

- [overview.md](overview.md): the summary, the invariants, the processes, and
  why the store is fjall with a SQLite index.
- [library.md](library.md): the modules of the `atuin-desktop-capture` library.
- [reading.md](reading.md): what capture reads, the snapshot format, and when
  capture reads a window.
- [backends.md](backends.md): reading on each platform: AT-SPI, Wayland, the AX
  API, and where toolkit-specific code lives.
- [storage.md](storage.md): storing snapshots, the store's generations,
  deletion, and eviction.
- [setup.md](setup.md): what setup does on every platform, on Linux, and on
  macOS.
- [environments.md](environments.md): what each Linux environment and macOS
  version provides, which is what setup is based on.
- [privacy.md](privacy.md): what capture leaves out and why, and private
  browser windows.
- [performance.md](performance.md): the cost of reads, and where capture was
  tested.

The figures in these documents come from experiments with the prototype and
from the [harness](../../harness/README.md). Each one describes the
applications and the builds that were measured, and the conditions are given
with it.

[The guide](../guide.md) describes how to use desktop capture, and [the
roadmap](../roadmap.md) describes what comes next.
