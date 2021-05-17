use futures::executor::block_on;
use futures::future::{join_all, ready, Future, JoinAll};
use std::fmt::Debug;

fn assert_done<T, F>(actual_fut: F, expected: T)
where
    T: PartialEq + Debug,
    F: FnOnce() -> Box<dyn Future<Output = T> + Unpin>,
{
    let output = block_on(actual_fut());
    assert_eq!(output, expected);
}

#[test]
fn collect_collects() {
    assert_done(|| Box::new(join_all(vec![ready(1), ready(2)])), vec![1, 2]);
    assert_done(|| Box::new(join_all(vec![ready(1)])), vec![1]);
    // REVIEW: should this be implemented?
    // assert_done(|| Box::new(join_all(Vec::<i32>::new())), vec![]);

    // TODO: needs more tests
}

#[test]
fn join_all_iter_lifetime() {
    // In futures-rs version 0.1, this function would fail to typecheck due to an overly
    // conservative type parameterization of `JoinAll`.
    fn sizes(bufs: Vec<&[u8]>) -> Box<dyn Future<Output = Vec<usize>> + Unpin> {
        let iter = bufs.into_iter().map(|b| ready::<usize>(b.len()));
        Box::new(join_all(iter))
    }

    assert_done(|| sizes(vec![&[1, 2, 3], &[], &[0]]), vec![3_usize, 0, 1]);
}

#[test]
fn join_all_from_iter() {
    assert_done(
        || Box::new(vec![ready(1), ready(2)].into_iter().collect::<JoinAll<_>>()),
        vec![1, 2],
    )
}
