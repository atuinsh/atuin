// These tests require the thread-local scoped dispatcher, which only works when
// we have a standard library. The behaviour being tested should be the same
// with the standard lib disabled.
//
// The alternative would be for each of these tests to be defined in a separate
// file, which is :(
#![cfg(feature = "std")]

#[macro_use]
extern crate tracing;
use tracing::{
    span,
    subscriber::{with_default, Interest, Subscriber},
    Event, Level, Metadata,
};

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[test]
fn event_macros_dont_infinite_loop() {
    // This test ensures that an event macro within a subscriber
    // won't cause an infinite loop of events.
    struct TestSubscriber;
    impl Subscriber for TestSubscriber {
        fn register_callsite(&self, _: &Metadata<'_>) -> Interest {
            // Always return sometimes so that `enabled` will be called
            // (which can loop).
            Interest::sometimes()
        }

        fn enabled(&self, meta: &Metadata<'_>) -> bool {
            assert!(meta.fields().iter().any(|f| f.name() == "foo"));
            event!(Level::TRACE, bar = false);
            true
        }

        fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(0xAAAA)
        }

        fn record(&self, _: &span::Id, _: &span::Record<'_>) {}

        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}

        fn event(&self, event: &Event<'_>) {
            assert!(event.metadata().fields().iter().any(|f| f.name() == "foo"));
            event!(Level::TRACE, baz = false);
        }

        fn enter(&self, _: &span::Id) {}

        fn exit(&self, _: &span::Id) {}
    }

    with_default(TestSubscriber, || {
        event!(Level::TRACE, foo = false);
    })
}
