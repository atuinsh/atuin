/// Extracts the successful type of a `Poll<T>`.
///
/// This macro bakes in propagation of `Pending` signals by returning early.
#[macro_export]
macro_rules! ready {
    ($e:expr $(,)?) => (match $e {
        $crate::__private::Poll::Ready(t) => t,
        $crate::__private::Poll::Pending =>
            return $crate::__private::Poll::Pending,
    })
}
