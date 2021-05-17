# tui-rs

[![Build Status](https://github.com/fdehau/tui-rs/workflows/CI/badge.svg)](https://github.com/fdehau/tui-rs/actions?query=workflow%3ACI+)
[![Crate Status](https://img.shields.io/crates/v/tui.svg)](https://crates.io/crates/tui)
[![Docs Status](https://docs.rs/tui/badge.svg)](https://docs.rs/crate/tui/)

<img src="./assets/demo.gif" alt="Demo cast under Linux Termite with Inconsolata font 12pt">

`tui-rs` is a [Rust](https://www.rust-lang.org) library to build rich terminal
user interfaces and dashboards. It is heavily inspired by the `Javascript`
library [blessed-contrib](https://github.com/yaronn/blessed-contrib) and the
`Go` library [termui](https://github.com/gizak/termui).

The library itself supports four different backends to draw to the terminal. You
can either choose from:

  - [termion](https://github.com/ticki/termion)
  - [rustbox](https://github.com/gchp/rustbox)
  - [crossterm](https://github.com/crossterm-rs/crossterm)
  - [pancurses](https://github.com/ihalila/pancurses)

However, some features may only be available in one of the four.

The library is based on the principle of immediate rendering with intermediate
buffers. This means that at each new frame you should build all widgets that are
supposed to be part of the UI. While providing a great flexibility for rich and
interactive UI, this may introduce overhead for highly dynamic content. So, the
implementation try to minimize the number of ansi escapes sequences generated to
draw the updated UI. In practice, given the speed of `Rust` the overhead rather
comes from the terminal emulator than the library itself.

Moreover, the library does not provide any input handling nor any event system and
you may rely on the previously cited libraries to achieve such features.

### Rust version requirements

Since version 0.10.0, `tui` requires **rustc version 1.44.0 or greater**.

### [Documentation](https://docs.rs/tui)

### Demo

The demo shown in the gif can be run with all available backends
(`examples/*_demo.rs` files). For example to see the `termion` version one could
run:

```
cargo run --example termion_demo --release -- --tick-rate 200
```

where `tick-rate` is the UI refresh rate in ms.

The UI code is in [examples/demo/ui.rs](examples/demo/ui.rs) while the
application state is in [examples/demo/app.rs](examples/demo/app.rs).

Beware that the `termion_demo` only works on Unix platforms. If you are a Windows user,
you can see the same demo using the `crossterm` backend with the following command:

```
cargo run --example crossterm_demo --no-default-features --features="crossterm" --release -- --tick-rate 200
```

If the user interface contains glyphs that are not displayed correctly by your terminal, you may want to run
the demo without those symbols:

```
cargo run --example crossterm_demo --no-default-features --features="crossterm" --release -- --tick-rate 200 --enhanced-graphics false
```

### Widgets

The library comes with the following list of widgets:

  * [Block](examples/block.rs)
  * [Gauge](examples/gauge.rs)
  * [Sparkline](examples/sparkline.rs)
  * [Chart](examples/chart.rs)
  * [BarChart](examples/barchart.rs)
  * [List](examples/list.rs)
  * [Table](examples/table.rs)
  * [Paragraph](examples/paragraph.rs)
  * [Canvas (with line, point cloud, map)](examples/canvas.rs)
  * [Tabs](examples/tabs.rs)

Click on each item to see the source of the example. Run the examples with with 
cargo (e.g. to run the demo `cargo run --example demo`), and quit by pressing `q`.

You can run all examples by running `make run-examples`.

### Third-party widgets

* [tui-logger](https://github.com/gin66/tui-logger)

### Apps using tui

* [spotify-tui](https://github.com/Rigellute/spotify-tui)
* [bandwhich](https://github.com/imsnif/bandwhich)
* [kmon](https://github.com/orhun/kmon)
* [ytop](https://github.com/cjbassi/ytop)
* [zenith](https://github.com/bvaisvil/zenith)
* [bottom](https://github.com/ClementTsang/bottom)
* [oha](https://github.com/hatoo/oha)
* [gitui](https://github.com/extrawurst/gitui)
* [rust-sadari-cli](https://github.com/24seconds/rust-sadari-cli)
* [desed](https://github.com/SoptikHa2/desed)
* [diskonaut](https://github.com/imsnif/diskonaut)
* [tickrs](https://github.com/tarkah/tickrs)
* [rusty-krab-manager](https://github.com/aryakaul/rusty-krab-manager)
* [termchat](https://github.com/lemunozm/termchat)
* [taskwarrior-tui](https://github.com/kdheepak/taskwarrior-tui)
* [gping](https://github.com/orf/gping/)
* [Vector](https://vector.dev)

### Alternatives

You might want to checkout [Cursive](https://github.com/gyscos/Cursive) for an
alternative solution to build text user interfaces in Rust.

## License

[MIT](LICENSE)
