use futures::channel::mpsc;
use futures::executor::block_on_stream;
use futures::future::{self, FutureExt};
use futures::stream::{self, select_all, FusedStream, SelectAll, StreamExt};
use futures::task::Poll;
use futures_test::task::noop_context;

#[test]
fn is_terminated() {
    let mut cx = noop_context();
    let mut tasks = SelectAll::new();

    assert_eq!(tasks.is_terminated(), false);
    assert_eq!(tasks.poll_next_unpin(&mut cx), Poll::Ready(None));
    assert_eq!(tasks.is_terminated(), true);

    // Test that the sentinel value doesn't leak
    assert_eq!(tasks.is_empty(), true);
    assert_eq!(tasks.len(), 0);

    tasks.push(future::ready(1).into_stream());

    assert_eq!(tasks.is_empty(), false);
    assert_eq!(tasks.len(), 1);

    assert_eq!(tasks.is_terminated(), false);
    assert_eq!(tasks.poll_next_unpin(&mut cx), Poll::Ready(Some(1)));
    assert_eq!(tasks.is_terminated(), false);
    assert_eq!(tasks.poll_next_unpin(&mut cx), Poll::Ready(None));
    assert_eq!(tasks.is_terminated(), true);
}

#[test]
fn issue_1626() {
    let a = stream::iter(0..=2);
    let b = stream::iter(10..=14);

    let mut s = block_on_stream(stream::select_all(vec![a, b]));

    assert_eq!(s.next(), Some(0));
    assert_eq!(s.next(), Some(10));
    assert_eq!(s.next(), Some(1));
    assert_eq!(s.next(), Some(11));
    assert_eq!(s.next(), Some(2));
    assert_eq!(s.next(), Some(12));
    assert_eq!(s.next(), Some(13));
    assert_eq!(s.next(), Some(14));
    assert_eq!(s.next(), None);
}

#[test]
fn works_1() {
    let (a_tx, a_rx) = mpsc::unbounded::<u32>();
    let (b_tx, b_rx) = mpsc::unbounded::<u32>();
    let (c_tx, c_rx) = mpsc::unbounded::<u32>();

    let streams = vec![a_rx, b_rx, c_rx];

    let mut stream = block_on_stream(select_all(streams));

    b_tx.unbounded_send(99).unwrap();
    a_tx.unbounded_send(33).unwrap();
    assert_eq!(Some(33), stream.next());
    assert_eq!(Some(99), stream.next());

    b_tx.unbounded_send(99).unwrap();
    a_tx.unbounded_send(33).unwrap();
    assert_eq!(Some(33), stream.next());
    assert_eq!(Some(99), stream.next());

    c_tx.unbounded_send(42).unwrap();
    assert_eq!(Some(42), stream.next());
    a_tx.unbounded_send(43).unwrap();
    assert_eq!(Some(43), stream.next());

    drop((a_tx, b_tx, c_tx));
    assert_eq!(None, stream.next());
}
