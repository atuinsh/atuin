# ratatui

An actively maintained `tui`-rs fork.

[![Build Status](https://github.com/tui-rs-revival/ratatui/workflows/CI/badge.svg)](https://github.com/tui-rs-revival/ratatui/actions?query=workflow%3ACI+)
[![Crate Status](https://img.shields.io/crates/v/ratatui.svg)](https://crates.io/crates/ratatui)
[![Docs Status](https://docs.rs/ratatui/badge.svg)](https://docs.rs/crate/ratatui/)

<img src="./assets/demo.gif" alt="Demo cast under Linux Termite with Inconsolata font 12pt">

# Install

```toml
[dependencies]
tui = { package = "ratatui" }
```

# What is this fork?

This fork was created to continue maintenance on the original TUI project. The original maintainer had created an [issue](https://github.com/fdehau/tui-rs/issues/654) explaining how he couldn't find time to continue development, which led to us creating this fork.

With that in mind, **we the community** look forward to continuing the work started by [**Florian Dehau.**](https://github.com/fdehau) :rocket:

In order to organize ourselves, we currently use a [discord server](https://discord.gg/pMCEU9hNEj), feel free to join and come chat ! There are also plans to implement a [matrix](https://matrix.org/) bridge in the near future.
**Discord is not a MUST to contribute,** we follow a pretty standard github centered open source workflow keeping the most important conversations on github, open an issue or PR and it will be addressed. :smile:

Please make sure you read the updated contributing guidelines, especially if you are interested in working on a PR or issue opened in the previous repository.

# Introduction

`ratatui`-rs is a [Rust](https://www.rust-lang.org) library to build rich terminal
user interfaces and dashboards. It is heavily inspired by the `Javascript`
library [blessed-contrib](https://github.com/yaronn/blessed-contrib) and the
`Go` library [termui](https://github.com/gizak/termui).

The library supports multiple backends:

- [crossterm](https://github.com/crossterm-rs/crossterm) [default]
- [termion](https://github.com/ticki/termion)

The library is based on the principle of immediate rendering with intermediate
buffers. This means that at each new frame you should build all widgets that are
supposed to be part of the UI. While providing a great flexibility for rich and
interactive UI, this may introduce overhead for highly dynamic content. So, the
implementation try to minimize the number of ansi escapes sequences generated to
draw the updated UI. In practice, given the speed of `Rust` the overhead rather
comes from the terminal emulator than the library itself.

Moreover, the library does not provide any input handling nor any event system and
you may rely on the previously cited libraries to achieve such features.

## Rust version requirements

Since version 0.17.0, `ratatui` requires **rustc version 1.59.0 or greater**.

# Documentation

The documentation can be found on [docs.rs.](https://docs.rs/ratatui)

# Demo

The demo shown in the gif can be run with all available backends.

```
# crossterm
cargo run --example demo --release -- --tick-rate 200
# termion
cargo run --example demo --no-default-features --features=termion --release -- --tick-rate 200
```

where `tick-rate` is the UI refresh rate in ms.

The UI code is in [examples/demo/ui.rs](https://github.com/tui-rs-revival/ratatui/blob/main/examples/demo/ui.rs) while the
application state is in [examples/demo/app.rs](https://github.com/tui-rs-revival/ratatui/blob/main/examples/demo/app.rs).

If the user interface contains glyphs that are not displayed correctly by your terminal, you may want to run
the demo without those symbols:

```
cargo run --example demo --release -- --tick-rate 200 --enhanced-graphics false
```

# Widgets

## Built in

The library comes with the following list of widgets:

- [Block](https://github.com/tui-rs-revival/ratatui/blob/main/examples/block.rs)
- [Gauge](https://github.com/tui-rs-revival/ratatui/blob/main/examples/gauge.rs)
- [Sparkline](https://github.com/tui-rs-revival/ratatui/blob/main/examples/sparkline.rs)
- [Chart](https://github.com/tui-rs-revival/ratatui/blob/main/examples/chart.rs)
- [BarChart](https://github.com/tui-rs-revival/ratatui/blob/main/examples/barchart.rs)
- [List](https://github.com/tui-rs-revival/ratatui/blob/main/examples/list.rs)
- [Table](https://github.com/tui-rs-revival/ratatui/blob/main/examples/table.rs)
- [Paragraph](https://github.com/tui-rs-revival/ratatui/blob/main/examples/paragraph.rs)
- [Canvas (with line, point cloud, map)](https://github.com/tui-rs-revival/ratatui/blob/main/examples/canvas.rs)
- [Tabs](https://github.com/tui-rs-revival/ratatui/blob/main/examples/tabs.rs)

Click on each item to see the source of the example. Run the examples with with
cargo (e.g. to run the gauge example `cargo run --example gauge`), and quit by pressing `q`.

You can run all examples by running `cargo make run-examples` (require
`cargo-make` that can be installed with `cargo install cargo-make`).

### Third-party libraries, bootstrapping templates and widgets

- [ansi-to-tui](https://github.com/uttarayan21/ansi-to-tui) — Convert ansi colored text to `tui::text::Text`
- [color-to-tui](https://github.com/uttarayan21/color-to-tui) — Parse hex colors to `tui::style::Color`
- [rust-tui-template](https://github.com/orhun/rust-tui-template) — A template for bootstrapping a Rust TUI application with Tui-rs & crossterm
- [simple-tui-rs](https://github.com/pmsanford/simple-tui-rs) — A simple example tui-rs app
- [tui-builder](https://github.com/jkelleyrtp/tui-builder) — Batteries-included MVC framework for Tui-rs + Crossterm apps
- [tui-clap](https://github.com/kegesch/tui-clap-rs) — Use clap-rs together with Tui-rs
- [tui-log](https://github.com/kegesch/tui-log-rs) — Example of how to use logging with Tui-rs
- [tui-logger](https://github.com/gin66/tui-logger) — Logger and Widget for Tui-rs
- [tui-realm](https://github.com/veeso/tui-realm) — Tui-rs framework to build stateful applications with a React/Elm inspired approach
- [tui-realm-treeview](https://github.com/veeso/tui-realm-treeview) — Treeview component for Tui-realm
- [tui tree widget](https://github.com/EdJoPaTo/tui-rs-tree-widget) — Tree Widget for Tui-rs
- [tui-windows](https://github.com/markatk/tui-windows-rs) — Tui-rs abstraction to handle multiple windows and their rendering
- [tui-textarea](https://github.com/rhysd/tui-textarea): Simple yet powerful multi-line text editor widget supporting several key shortcuts, undo/redo, text search, etc.
- [tui-rs-tree-widgets](https://github.com/EdJoPaTo/tui-rs-tree-widget): Widget for tree data structures.
- [tui-input](https://github.com/sayanarijit/tui-input): TUI input library supporting multiple backends and tui-rs.

# Apps

Check out the list of [close to 40 apps](./APPS.md) using `ratatui`!

# Alternatives

You might want to checkout [Cursive](https://github.com/gyscos/Cursive) for an
alternative solution to build text user interfaces in Rust.

# License

[MIT](LICENSE)

