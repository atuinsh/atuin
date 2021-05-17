mod drop;

use self::drop::{DetectDrop, Flag};
use eyre::Report;
use std::marker::Unpin;
use std::mem;

#[test]
fn test_error_size() {
    assert_eq!(mem::size_of::<Report>(), mem::size_of::<usize>());
}

#[test]
fn test_null_pointer_optimization() {
    assert_eq!(
        mem::size_of::<Result<(), Report>>(),
        mem::size_of::<usize>()
    );
}

#[test]
fn test_autotraits() {
    fn assert<E: Unpin + Send + Sync + 'static>() {}
    assert::<Report>();
}

#[test]
fn test_drop() {
    let has_dropped = Flag::new();
    drop(Report::new(DetectDrop::new(&has_dropped)));
    assert!(has_dropped.get());
}
