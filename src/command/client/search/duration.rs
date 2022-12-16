use std::{ops::ControlFlow, time::Duration};

#[allow(clippy::module_name_repetitions)]
pub fn format_duration(f: Duration) -> String {
    fn item(name: &str, value: u64) -> ControlFlow<String> {
        if value > 0 {
            ControlFlow::Break(format!("{value}{name}"))
        } else {
            ControlFlow::Continue(())
        }
    }

    // impl taken and modified from
    // https://github.com/tailhook/humantime/blob/master/src/duration.rs#L295-L331
    // Copyright (c) 2016 The humantime Developers
    fn fmt(f: Duration) -> ControlFlow<String, ()> {
        let secs = f.as_secs();
        let nanos = f.subsec_nanos();

        let years = secs / 31_557_600; // 365.25d
        let year_days = secs % 31_557_600;
        let months = year_days / 2_630_016; // 30.44d
        let month_days = year_days % 2_630_016;
        let days = month_days / 86400;
        let day_secs = month_days % 86400;
        let hours = day_secs / 3600;
        let minutes = day_secs % 3600 / 60;
        let seconds = day_secs % 60;

        let millis = nanos / 1_000_000;

        // a difference from our impl than the original is that
        // we only care about the most-significant segment of the duration.
        // If the item call returns `Break`, then the `?` will early-return.
        // This allows for a very consise impl
        item("y", years)?;
        item("mo", months)?;
        item("d", days)?;
        item("h", hours)?;
        item("m", minutes)?;
        item("s", seconds)?;
        item("ms", u64::from(millis))?;
        ControlFlow::Continue(())
    }

    match fmt(f) {
        ControlFlow::Break(b) => b,
        ControlFlow::Continue(()) => String::from("0s"),
    }
}
