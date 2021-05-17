use futures::executor::block_on;
use futures_util::future::{err, ok, try_join_all, TryJoinAll};
use std::fmt::Debug;
use std::future::Future;

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
    assert_done(
        || Box::new(try_join_all(vec![ok(1), ok(2)])),
        Ok::<_, usize>(vec![1, 2]),
    );
    assert_done(|| Box::new(try_join_all(vec![ok(1), err(2)])), Err(2));
    assert_done(
        || Box::new(try_join_all(vec![ok(1)])),
        Ok::<_, usize>(vec![1]),
    );
    // REVIEW: should this be implemented?
    // assert_done(|| Box::new(try_join_all(Vec::<i32>::new())), Ok(vec![]));

    // TODO: needs more tests
}

#[test]
fn try_join_all_iter_lifetime() {
    // In futures-rs version 0.1, this function would fail to typecheck due to an overly
    // conservative type parameterization of `TryJoinAll`.
    fn sizes(bufs: Vec<&[u8]>) -> Box<dyn Future<Output = Result<Vec<usize>, ()>> + Unpin> {
        let iter = bufs.into_iter().map(|b| ok::<usize, ()>(b.len()));
        Box::new(try_join_all(iter))
    }

    assert_done(
        || sizes(vec![&[1, 2, 3], &[], &[0]]),
        Ok(vec![3_usize, 0, 1]),
    );
}

#[test]
fn try_join_all_from_iter() {
    assert_done(
        || Box::new(vec![ok(1), ok(2)].into_iter().collect::<TryJoinAll<_>>()),
        Ok::<_, usize>(vec![1, 2]),
    )
}
