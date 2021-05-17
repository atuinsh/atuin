use futures::channel::mpsc;
use futures::executor::block_on;
use futures::future::{self, Future};
use futures::sink::SinkExt;
use futures::stream::{self, StreamExt};
use futures::task::Poll;
use futures::FutureExt;
use futures_test::task::noop_context;

#[test]
fn select() {
    fn select_and_compare(a: Vec<u32>, b: Vec<u32>, expected: Vec<u32>) {
        let a = stream::iter(a);
        let b = stream::iter(b);
        let vec = block_on(stream::select(a, b).collect::<Vec<_>>());
        assert_eq!(vec, expected);
    }

    select_and_compare(vec![1, 2, 3], vec![4, 5, 6], vec![1, 4, 2, 5, 3, 6]);
    select_and_compare(vec![1, 2, 3], vec![4, 5], vec![1, 4, 2, 5, 3]);
    select_and_compare(vec![1, 2], vec![4, 5, 6], vec![1, 4, 2, 5, 6]);
}

#[test]
fn flat_map() {
    block_on(async {
        let st = stream::iter(vec![
            stream::iter(0..=4u8),
            stream::iter(6..=10),
            stream::iter(0..=2),
        ]);

        let values: Vec<_> = st
            .flat_map(|s| s.filter(|v| futures::future::ready(v % 2 == 0)))
            .collect()
            .await;

        assert_eq!(values, vec![0, 2, 4, 6, 8, 10, 0, 2]);
    });
}

#[test]
fn scan() {
    block_on(async {
        let values = stream::iter(vec![1u8, 2, 3, 4, 6, 8, 2])
            .scan(1, |state, e| {
                *state += 1;
                futures::future::ready(if e < *state { Some(e) } else { None })
            })
            .collect::<Vec<_>>()
            .await;

        assert_eq!(values, vec![1u8, 2, 3, 4]);
    });
}

#[test]
fn take_until() {
    fn make_stop_fut(stop_on: u32) -> impl Future<Output = ()> {
        let mut i = 0;
        future::poll_fn(move |_cx| {
            i += 1;
            if i <= stop_on {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
    }

    block_on(async {
        // Verify stopping works:
        let stream = stream::iter(1u32..=10);
        let stop_fut = make_stop_fut(5);

        let stream = stream.take_until(stop_fut);
        let last = stream.fold(0, |_, i| async move { i }).await;
        assert_eq!(last, 5);

        // Verify take_future() works:
        let stream = stream::iter(1..=10);
        let stop_fut = make_stop_fut(5);

        let mut stream = stream.take_until(stop_fut);

        assert_eq!(stream.next().await, Some(1));
        assert_eq!(stream.next().await, Some(2));

        stream.take_future();

        let last = stream.fold(0, |_, i| async move { i }).await;
        assert_eq!(last, 10);

        // Verify take_future() returns None if stream is stopped:
        let stream = stream::iter(1u32..=10);
        let stop_fut = make_stop_fut(1);
        let mut stream = stream.take_until(stop_fut);
        assert_eq!(stream.next().await, Some(1));
        assert_eq!(stream.next().await, None);
        assert!(stream.take_future().is_none());

        // Verify TakeUntil is fused:
        let mut i = 0;
        let stream = stream::poll_fn(move |_cx| {
            i += 1;
            match i {
                1 => Poll::Ready(Some(1)),
                2 => Poll::Ready(None),
                _ => panic!("TakeUntil not fused"),
            }
        });

        let stop_fut = make_stop_fut(1);
        let mut stream = stream.take_until(stop_fut);
        assert_eq!(stream.next().await, Some(1));
        assert_eq!(stream.next().await, None);
        assert_eq!(stream.next().await, None);
    });
}

#[test]
#[should_panic]
fn chunks_panic_on_cap_zero() {
    let (_, rx1) = mpsc::channel::<()>(1);

    let _ = rx1.chunks(0);
}

#[test]
#[should_panic]
fn ready_chunks_panic_on_cap_zero() {
    let (_, rx1) = mpsc::channel::<()>(1);

    let _ = rx1.ready_chunks(0);
}

#[test]
fn ready_chunks() {
    let (mut tx, rx1) = mpsc::channel::<i32>(16);

    let mut s = rx1.ready_chunks(2);

    let mut cx = noop_context();
    assert!(s.next().poll_unpin(&mut cx).is_pending());

    block_on(async {
        tx.send(1).await.unwrap();

        assert_eq!(s.next().await.unwrap(), vec![1]);
        tx.send(2).await.unwrap();
        tx.send(3).await.unwrap();
        tx.send(4).await.unwrap();
        assert_eq!(s.next().await.unwrap(), vec![2, 3]);
        assert_eq!(s.next().await.unwrap(), vec![4]);
    });
}
