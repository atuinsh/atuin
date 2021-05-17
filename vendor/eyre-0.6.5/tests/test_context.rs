mod drop;

use crate::drop::{DetectDrop, Flag};
use eyre::{Report, Result, WrapErr};
use std::fmt::{self, Display};
use thiserror::Error;

// https://github.com/dtolnay/eyre/issues/18
#[test]
fn test_inference() -> Result<()> {
    let x = "1";
    let y: u32 = x.parse().wrap_err("...")?;
    assert_eq!(y, 1);
    Ok(())
}

macro_rules! context_type {
    ($name:ident) => {
        #[derive(Debug)]
        struct $name {
            message: &'static str,
            drop: DetectDrop,
        }

        impl Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(self.message)
            }
        }
    };
}

context_type!(HighLevel);
context_type!(MidLevel);

#[derive(Error, Debug)]
#[error("{message}")]
struct LowLevel {
    message: &'static str,
    drop: DetectDrop,
}

struct Dropped {
    low: Flag,
    mid: Flag,
    high: Flag,
}

impl Dropped {
    fn none(&self) -> bool {
        !self.low.get() && !self.mid.get() && !self.high.get()
    }

    fn all(&self) -> bool {
        self.low.get() && self.mid.get() && self.high.get()
    }
}

fn make_chain() -> (Report, Dropped) {
    let dropped = Dropped {
        low: Flag::new(),
        mid: Flag::new(),
        high: Flag::new(),
    };

    let low = LowLevel {
        message: "no such file or directory",
        drop: DetectDrop::new(&dropped.low),
    };

    // impl Report for Result<T, E>
    let mid = Err::<(), LowLevel>(low)
        .wrap_err(MidLevel {
            message: "failed to load config",
            drop: DetectDrop::new(&dropped.mid),
        })
        .unwrap_err();

    // impl Report for Result<T, Error>
    let high = Err::<(), Report>(mid)
        .wrap_err(HighLevel {
            message: "failed to start server",
            drop: DetectDrop::new(&dropped.high),
        })
        .unwrap_err();

    (high, dropped)
}

#[test]
fn test_downcast_ref() {
    let (err, dropped) = make_chain();

    assert!(!err.is::<String>());
    assert!(err.downcast_ref::<String>().is_none());

    assert!(err.is::<HighLevel>());
    let high = err.downcast_ref::<HighLevel>().unwrap();
    assert_eq!(high.to_string(), "failed to start server");

    assert!(err.is::<MidLevel>());
    let mid = err.downcast_ref::<MidLevel>().unwrap();
    assert_eq!(mid.to_string(), "failed to load config");

    assert!(err.is::<LowLevel>());
    let low = err.downcast_ref::<LowLevel>().unwrap();
    assert_eq!(low.to_string(), "no such file or directory");

    assert!(dropped.none());
    drop(err);
    assert!(dropped.all());
}

#[test]
fn test_downcast_high() {
    let (err, dropped) = make_chain();

    let err = err.downcast::<HighLevel>().unwrap();
    assert!(!dropped.high.get());
    assert!(dropped.low.get() && dropped.mid.get());

    drop(err);
    assert!(dropped.all());
}

#[test]
fn test_downcast_mid() {
    let (err, dropped) = make_chain();

    let err = err.downcast::<MidLevel>().unwrap();
    assert!(!dropped.mid.get());
    assert!(dropped.low.get() && dropped.high.get());

    drop(err);
    assert!(dropped.all());
}

#[test]
fn test_downcast_low() {
    let (err, dropped) = make_chain();

    let err = err.downcast::<LowLevel>().unwrap();
    assert!(!dropped.low.get());
    assert!(dropped.mid.get() && dropped.high.get());

    drop(err);
    assert!(dropped.all());
}

#[test]
fn test_unsuccessful_downcast() {
    let (err, dropped) = make_chain();

    let err = err.downcast::<String>().unwrap_err();
    assert!(dropped.none());

    drop(err);
    assert!(dropped.all());
}
