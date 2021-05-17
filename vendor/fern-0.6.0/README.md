fern
====
[![Linux Build Status][travis-image]][travis-builds]
[![Windows Build Status][appveyor-image]][appveyor-builds]
[![Coverage Status][coveralls-badge]][coveralls-builds]

[![crates.io version badge][cratesio-badge]][fern-crate]

Simple, efficient logging for [Rust].

---

Logging configuration is recursively branched, like a fern: formatting, filters, and output can be applied recursively to match increasingly specific kinds of logging. Fern provides a builder-based configuration backing for rust's standard [log] crate.

```rust
//! With fern, we can:

// Configure logger at runtime
fern::Dispatch::new()
    // Perform allocation-free log formatting
    .format(|out, message, record| {
        out.finish(format_args!(
            "{}[{}][{}] {}",
            chrono::Local::now().format("[%Y-%m-%d][%H:%M:%S]"),
            record.target(),
            record.level(),
            message
        ))
    })
    // Add blanket level filter -
    .level(log::LevelFilter::Debug)
    // - and per-module overrides
    .level_for("hyper", log::LevelFilter::Info)
    // Output to stdout, files, and other Dispatch configurations
    .chain(std::io::stdout())
    .chain(fern::log_file("output.log")?)
    // Apply globally
    .apply()?;

// and log using log crate macros!
info!("hello, world!");
```

Examples of all features at the [api docs][fern-docs]. See fern in use with this [example command line program][fern-example].

---

- [documentation][fern-docs]
- [crates.io page][fern-crate]
- [example program][fern-example]

### Project Status

The fern project is primarily maintained by myself, @daboross on GitHub. It's a hobby project, but one I aim to keep at a high quality.

### Contributing

As this is a hobby project, contributions are very welcome!

The easiest way for you to contribute right now is to use fern in your application, and see where it's lacking. The current library has a solid base, but it lacks features, and I may not anticipate your use cases.

If you have a use case fern does not cover, please file an issue. This is immensely useful to me, to anyone wanting to contribute to the project, and to you as well if the feature is implemented.

If you're interested in helping fix an [existing issue](https://github.com/daboross/fern/issues), or an issue you just filed, help is appreciated.

See [CONTRIBUTING](./CONTRIBUTING.md) for technical information on contributing.

[Rust]: https://www.rust-lang.org/
[travis-image]: https://travis-ci.org/daboross/fern.svg?branch=master
[travis-builds]: https://travis-ci.org/daboross/fern
[appveyor-image]: https://ci.appveyor.com/api/projects/status/ofdv9657k88jbpel/branch/master?svg=true
[appveyor-image]: https://ci.appveyor.com/api/projects/status/github/daboross/fern?branch=master&svg=true
[appveyor-builds]: https://ci.appveyor.com/project/daboross/fern
[cratesio-badge]: http://meritbadge.herokuapp.com/fern
[coveralls-badge]: https://coveralls.io/repos/github/daboross/fern/badge.svg
[coveralls-builds]: https://coveralls.io/github/daboross/fern
[fern-docs]: https://docs.rs/fern/
[fern-crate]: https://crates.io/crates/fern
[fern-example]: https://github.com/daboross/fern/tree/master/examples/cmd-program.rs
[log]: https://github.com/rust-lang-nursery/log
