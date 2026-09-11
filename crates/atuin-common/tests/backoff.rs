//! Timing attacks on [`atuin_common::futures::Backoff`], the retry primitive the daemon's sync
//! worker relies on. A user feels a broken backoff two ways: "sync hammered the server" (retries
//! fired too fast / back-to-back / above the ceiling) or "sync gave up too early / never" (the
//! timeout and value/reason threading are wrong). Every test states the invariant a human relies on
//! and asserts virtual-time elapsed in RANGES that absorb the engine's +/-10% jitter, so a correct
//! run is never flaky. Time is driven by `start_paused`, so a "10s" sleep costs no wall-clock time.
//!
//! Nothing here is `#[ignore]`d: these behaviors are all correct-as-designed, and these tests are
//! the guard rail that keeps them so. The one counterintuitive behavior -- the eager first call
//! running OUTSIDE the timeout -- is documented ("Be warned") and pinned as a passing guard, not a
//! defect.

use std::future::ready;
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use atuin_common::futures::Backoff;
use tokio::time::Instant;

const SEC: Duration = Duration::from_secs(1);

/// A backoff delay is jittered by up to +/-10%; assert the observed gap lands in that band. Uses a
/// slightly wider `0.89..=1.11` window than the nominal `0.9..=1.1` so integer rounding in the
/// engine's jitter arithmetic never trips a correct run.
#[track_caller]
fn assert_in_band(actual: Duration, expected: Duration, label: &str) {
    let lo = expected.mul_f64(0.89);
    let hi = expected.mul_f64(1.11);
    assert!(
        actual >= lo && actual <= hi,
        "{label}: {actual:?} is outside the +/-10% jitter band of {expected:?} ({lo:?}..={hi:?})"
    );
}

/// The virtual instant, relative to `start`, of each consecutive attempt.
fn gaps(stamps: &[Duration]) -> Vec<Duration> {
    stamps.windows(2).map(|w| w[1] - w[0]).collect()
}

/// Drive `backoff`, returning `Continue` for `fails` attempts and then `Break`, recording the
/// elapsed virtual time at the moment of every attempt. Exercises the synchronous entry point.
async fn record(backoff: Backoff, fails: usize, timeout: Duration) -> Vec<Duration> {
    let start = Instant::now();
    let mut stamps = Vec::new();
    let _: Result<(), ()> = backoff
        .retry_sync(
            || {
                stamps.push(start.elapsed());
                if stamps.len() > fails {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            },
            timeout,
        )
        .await;
    stamps
}

/// The first call is eager and awaited OUTSIDE the timeout, so an episode can run far past its
/// `timeout` when that first attempt is slow -- exactly what the doc warns about. A user must not
/// assume `timeout` bounds total wall time: a single slow attempt blows through it.
#[tokio::test(start_paused = true)]
async fn eager_first_call_can_exceed_the_timeout() {
    let first_call_cost = Duration::from_secs(100);
    let timeout = Duration::from_millis(1);
    let calls = AtomicUsize::new(0);

    let start = Instant::now();
    let result: Result<(), ()> = Backoff::Linear(Duration::from_secs(10))
        .retry(
            || async {
                calls.fetch_add(1, Ordering::SeqCst);
                // The first (and here only) attempt is not wrapped by the timeout.
                tokio::time::sleep(first_call_cost).await;
                ControlFlow::Continue(())
            },
            timeout,
        )
        .await;

    // The eager attempt ran to completion despite the 1ms budget; the loop's first backoff sleep
    // then tripped the timeout, so the reason from that one attempt comes back as the error.
    assert!(result.is_err(), "a never-succeeding episode must time out");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "only the eager attempt ran before the timeout");
    assert!(
        start.elapsed() >= first_call_cost,
        "episode returned in {:?}, but the eager first call alone costs {first_call_cost:?} and \
         is outside the {timeout:?} budget",
        start.elapsed()
    );
}

/// Only the eager first attempt is un-delayed; every retry after it waits a full, jittered backoff.
/// This is the "sync must not grow ever more aggressive" guarantee -- successive delays climb
/// geometrically (`initial`, `initial*factor`, ...), never collapsing back toward zero.
#[tokio::test(start_paused = true)]
async fn exponential_backoff_grows_geometrically() {
    let backoff = Backoff::Exponential {
        initial: SEC,
        max: Duration::from_secs(1000),
        factor: NonZeroU32::new(2).unwrap(),
    };
    let stamps = record(backoff, 5, Duration::from_secs(10_000)).await;

    assert_eq!(stamps.len(), 6, "expected five failures then a break");
    assert_eq!(stamps[0], Duration::ZERO, "the first attempt must fire eagerly, with no delay");

    let observed = gaps(&stamps);
    let expected = [SEC, 2 * SEC, 4 * SEC, 8 * SEC, 16 * SEC];
    for (i, (got, want)) in observed.iter().zip(expected).enumerate() {
        assert_in_band(*got, want, &format!("backoff #{}", i + 1));
    }
}

/// Exponential backoff SATURATES at `max`: no single delay ever exceeds the configured ceiling,
/// even after the raw geometric series has rocketed far past it. This is the hard cap that stops a
/// long outage from stretching the retry interval to hours -- sync keeps probing at `~max`.
#[tokio::test(start_paused = true)]
async fn exponential_backoff_never_exceeds_max() {
    let max = 5 * SEC;
    let backoff = Backoff::Exponential {
        initial: SEC,
        max,
        // A factor of 10 blows past `max` after a single step, so most delays are saturated.
        factor: NonZeroU32::new(10).unwrap(),
    };
    let stamps = record(backoff, 7, Duration::from_secs(10_000)).await;
    let observed = gaps(&stamps);

    for (i, got) in observed.iter().enumerate() {
        assert!(*got <= max, "backoff #{} was {got:?}, above the {max:?} ceiling", i + 1);
    }
    // Everything from the second delay on is saturated and should sit right below the ceiling.
    for (i, got) in observed.iter().enumerate().skip(1) {
        assert_in_band(*got, max, &format!("saturated backoff #{}", i + 1));
    }
}

/// An `initial` larger than `max` is capped to `max` from the very first delay -- a misconfiguration
/// can only make sync poll at most every `max`, never the (nonsensical) larger `initial`.
#[tokio::test(start_paused = true)]
async fn exponential_initial_is_capped_to_max() {
    let max = 10 * SEC;
    let backoff = Backoff::Exponential {
        initial: 100 * SEC, // ten times the ceiling
        max,
        factor: NonZeroU32::new(2).unwrap(),
    };
    let stamps = record(backoff, 1, Duration::from_secs(10_000)).await;
    let observed = gaps(&stamps);

    assert_eq!(observed.len(), 1, "expected one failure then a break");
    // If `initial` were not capped this would be ~100s, not ~10s.
    assert_in_band(observed[0], max, "first delay when initial > max");
}

/// `Linear(ZERO)` spins -- it polls as fast as possible with no delay -- yet still terminates the
/// instant the closure returns `Break`. Guards the documented busy-poll mode against both a hidden
/// delay creeping in and an infinite loop that never honors `Break`.
#[tokio::test(start_paused = true)]
async fn linear_zero_spins_with_no_delay() {
    let calls = AtomicUsize::new(0);
    let start = Instant::now();
    let result: Result<usize, ()> = Backoff::Linear(Duration::ZERO)
        .retry_sync(
            || {
                let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                if n >= 500 {
                    ControlFlow::Break(n)
                } else {
                    ControlFlow::Continue(())
                }
            },
            Duration::from_secs(10_000),
        )
        .await;

    assert_eq!(result, Ok(500), "spinning must still honor Break and thread its value");
    assert_eq!(calls.load(Ordering::SeqCst), 500);
    assert_eq!(start.elapsed(), Duration::ZERO, "a ZERO linear backoff must insert no delay");
}

/// `Linear(period)` waits ~`period` between EVERY pair of attempts, across many failures -- not just
/// the first. This is the "sync never hammers the server" guarantee: two attempts never fire
/// back-to-back, and the cadence never drifts as failures pile up.
#[tokio::test(start_paused = true)]
async fn linear_waits_its_period_between_attempts() {
    let period = 2 * SEC;
    let stamps = record(Backoff::Linear(period), 5, Duration::from_secs(10_000)).await;

    assert_eq!(stamps.len(), 6, "expected five failures then a break");
    assert_eq!(stamps[0], Duration::ZERO, "only the eager first attempt is un-delayed");

    for (i, got) in gaps(&stamps).iter().enumerate() {
        assert_in_band(*got, period, &format!("linear gap #{}", i + 1));
    }
}

/// When the timeout elapses mid-backoff, the error carries the LATEST `Continue` reason -- so a
/// caller that gives up learns why the most recent attempt failed, not a stale earlier reason.
#[tokio::test(start_paused = true)]
async fn timeout_mid_backoff_returns_the_last_continue_reason() {
    let calls = AtomicUsize::new(0);
    // Attempts land at ~0s, ~10s, ~20s (reasons 0, 1, 2); the fourth backoff would reach ~30s but
    // the 25s timeout fires first, so the error must carry the reason from attempt #3.
    let result: Result<(), usize> = Backoff::Linear(10 * SEC)
        .retry_sync(|| ControlFlow::Continue(calls.fetch_add(1, Ordering::SeqCst)), 25 * SEC)
        .await;

    assert_eq!(result, Err(2), "timeout must surface the most recent Continue reason");
    assert_eq!(calls.load(Ordering::SeqCst), 3, "exactly three attempts fit inside the timeout");
}

/// A `Break` that lands just before the timeout wins: the episode returns `Ok` with the break's
/// value, not a timeout error. Distinguishing "succeeded at the last moment" from "gave up" is the
/// difference between sync completing and sync silently failing.
#[tokio::test(start_paused = true)]
async fn break_before_timeout_returns_the_ok_value() {
    let calls = AtomicUsize::new(0);
    // Attempts at ~0s, ~10s, ~20s; the third breaks at ~20s, comfortably inside the 25s timeout.
    let result: Result<&str, usize> = Backoff::Linear(10 * SEC)
        .retry_sync(
            || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                if n >= 2 {
                    ControlFlow::Break("done")
                } else {
                    ControlFlow::Continue(n)
                }
            },
            25 * SEC,
        )
        .await;

    assert_eq!(result, Ok("done"), "a break inside the budget must return Ok(value), not time out");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

/// `retry_sync` is `retry` for a synchronous closure: identical timing, identical result threading.
/// A caller must be able to reach for whichever fits without changing behavior.
#[tokio::test(start_paused = true)]
async fn retry_sync_matches_retry() {
    let backoff = Backoff::Exponential {
        initial: 2 * SEC,
        max: Duration::from_secs(1000),
        factor: NonZeroU32::new(3).unwrap(),
    };
    let timeout = Duration::from_secs(10_000);
    let expected = [2 * SEC, 6 * SEC]; // initial, initial*factor

    // Synchronous path.
    let sync_start = Instant::now();
    let mut sync_stamps = Vec::new();
    let sync_result: Result<&str, ()> = backoff
        .retry_sync(
            || {
                sync_stamps.push(sync_start.elapsed());
                if sync_stamps.len() > 2 {
                    ControlFlow::Break("ok")
                } else {
                    ControlFlow::Continue(())
                }
            },
            timeout,
        )
        .await;

    // Asynchronous path, same scenario driven through ready futures.
    let async_start = Instant::now();
    let mut async_stamps = Vec::new();
    let async_result: Result<&str, ()> = backoff
        .retry(
            || {
                async_stamps.push(async_start.elapsed());
                ready(if async_stamps.len() > 2 {
                    ControlFlow::Break("ok")
                } else {
                    ControlFlow::Continue(())
                })
            },
            timeout,
        )
        .await;

    assert_eq!(sync_result, Ok("ok"));
    assert_eq!(async_result, sync_result, "retry_sync and retry must return the same value");
    for (path, stamps) in [("sync", &sync_stamps), ("async", &async_stamps)] {
        assert_eq!(stamps.len(), 3, "{path}: two failures then a break");
        assert_eq!(stamps[0], Duration::ZERO, "{path}: eager first attempt");
        for (i, (got, want)) in gaps(stamps).iter().zip(expected).enumerate() {
            assert_in_band(*got, want, &format!("{path} backoff #{}", i + 1));
        }
    }
}
