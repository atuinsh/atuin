/*!
Example usage of `fern` with the `syslog` crate.

Be sure to depend on `syslog` and the `syslog` feature in `Cargo.toml`:

```toml
[dependencies]
fern = { version = "0.5", features = ["syslog-4"] }]
syslog = "4"
```

To use `syslog`, simply create the log you want, and pass it into `Dispatch::chain`:

```no_run
# use syslog4 as syslog;
# fn setup_logging() -> Result<(), Box<dyn std::error::Error>> {
let formatter = syslog::Formatter3164 {
    facility: syslog::Facility::LOG_USER,
    hostname: None,
    process: "hello-world".to_owned(),
    pid: 0,
};

fern::Dispatch::new()
    .chain(syslog::unix(formatter)?)
    .apply()?;
# Ok(())
# }
# fn main() { setup_logging().ok(); }
```

---

## Alternate syslog versions

If you're using syslog=4.0.0 exactly, one line "ok" will be printed to stdout on log configuration.
This is [a bug in syslog](https://github.com/Geal/rust-syslog/issues/39), and there is nothing we
can change in fern to fix that.

One way to avoid this is to use an earlier version of syslog, which `fern` also supports. To do
this, depend on `syslog = 3` instead.

```toml
[dependencies]
fern = { version = "0.5", features = ["syslog-3"] }]
syslog = "3"
```

The setup is very similar, except with less configuration to start the syslog logger:

```rust
# use syslog3 as syslog;
# fn setup_logging() -> Result<(), Box<dyn std::error::Error>> {
fern::Dispatch::new()
    .chain(syslog::unix(syslog::Facility::LOG_USER)?)
    .apply()?;
# Ok(())
# }
# fn main() { setup_logging().ok(); }
```

The rest of this document applies to both syslog 3 and syslog 4, but the examples will be using
syslog 4 as it is the latest version.

---

One thing with `syslog` is that you don't generally want to apply any log formatting. The system
logger will handle that for you.

However, you probably will want to format messages you also send to stdout! Fortunately, selective
configuration is easy with fern:

```no_run
# use syslog4 as syslog;
# fn setup_logging() -> Result<(), Box<dyn std::error::Error>> {
let syslog_formatter = syslog::Formatter3164 {
    facility: syslog::Facility::LOG_USER,
    hostname: None,
    process: "hello-world".to_owned(),
    pid: 0,
};

// top level config
fern::Dispatch::new()
    .chain(
        // console config
        fern::Dispatch::new()
            .level(log::LevelFilter::Debug)
            .format(move |out, message, record| {
                out.finish(format_args!(
                    "[{}] {}",
                    record.level(),
                    message,
                ))
            })
            .chain(std::io::stdout())
    )
    .chain(
        // syslog config
        fern::Dispatch::new()
            .level(log::LevelFilter::Info)
            .chain(syslog::unix(syslog_formatter)?)
    )
    .apply()?;
# Ok(())
# }
# fn main() { setup_logging().ok(); }
```

With this, all info and above messages will be sent to the syslog with no formatting, and
the messages sent to the console will still look nice as usual.

---

One last pattern you might want to know: creating a log target which must be explicitly mentioned
in order to work.

```no_run
# use syslog4 as syslog;
# fn setup_logging() -> Result<(), Box<dyn std::error::Error>> {
# let formatter = syslog::Formatter3164 {
#     facility: syslog::Facility::LOG_USER,
#     hostname: None,
#     process: "hello-world".to_owned(),
#     pid: 0,
# };
fern::Dispatch::new()
    // by default only accept warning messages from libraries so we don't spam
    .level(log::LevelFilter::Warn)
    // but accept Info and Debug if we explicitly mention syslog
    .level_for("explicit-syslog", log::LevelFilter::Debug)
    .chain(syslog::unix(formatter)?)
    .apply()?;
# Ok(())
# }
# fn main() { setup_logging().ok(); }
```

With this configuration, only warning messages will get through by default. If we do want to
send info or debug messages, we can do so explicitly:

```no_run
# use log::{debug, info, warn};
# fn main() {
debug!("this won't get through");
// especially useful if this is from library you depend on.
info!("neither will this");
warn!("this will!");

info!(target: "explicit-syslog", "this will also show up!");
# }
```
*/
