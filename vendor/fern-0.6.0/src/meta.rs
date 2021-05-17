/*!
Fern supports logging most things by default, except for one kind of struct: structs which make log
calls to the global logger from within their `Display` or `Debug` implementations.

Here's an example of such a structure:

```
# use log::debug;
# use std::fmt;
#
struct Thing<'a>(&'a str);

impl<'a> fmt::Display for Thing<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        debug!("just displayed a Thing wrapping {}", self.0);
        f.write_str(self.0)
    }
}

# fn main() {}
```

This structure, and this structure alone, will cause some problems when logging in fern. There are
mitigations, but since it's a fairly niche use case, they are disabled by default.

The problems are, depending on which backend you use:

- stdout/stderr: logging will 'stutter', with the logs output inside the `Display` implementation
  cutting other log lines down the center
- file: thread will deadlock, and all future logs will also deadlock

There are two mitigations you can make, both completely fix this error.

The simplest mitigation to this is to enable the `meta-logging-in-format` feature of `fern`. The
disadvantage is that this means fern makes an additional allocation per log call per affected
backend. Not a huge cost, but enough to mean it's disabled by default. To enable this, use the
following in your `Cargo.toml`:

```toml
[dependencies]
# ...
fern = { version = "0.5", features = ["meta-logging-in-format"] }
```

The second mitigation is one you can make inside a formatting closure. This means extra code
complexity, but it also means you can enable it per-logger: the fix isn't global. This fix is also
redundant if you've already enable the above feature. To add the second mitigation, replacec
`format_args!()` with `format!()` as displayed below:

```
fern::Dispatch::new()
    # /*
    ...
    # */
    // instead of doing this:
    .format(move |out, message, record| {
        out.finish(format_args!("[{}] {}", record.level(), message))
    })
    // do this:
    .format(move |out, message, record| {
        let formatted = format!("[{}] {}", record.level(), message);

        out.finish(format_args!("{}", formatted))
    })
# ;
```

This second mitigation works by forcing the `Display` implementation to run before any text has
started to log to the backend. There's an additional allocation per log, but it no longer deadlocks!

This mitigation also has the advantage of ensuring there's only one call to `Display::fmt`. If youc
use `meta-logging-in-format` and have multiple backends, `Display::fmt` will still be called once
per backend. With this, it will only be called once.

------

If you've never experienced this problem, there's no need to fix it - `Display::fmt` and
`Debug::fmt` are normally implemented as "pure" functions with no side effects.
*/
