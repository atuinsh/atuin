//! Helpers for the fetches that a read runs at the same time.

use futures::future::OptionFuture;

/// Runs `fetch` when there is one.
pub async fn optional<T>(fetch: Option<impl Future<Output = Option<T>>>) -> Option<T> {
    OptionFuture::from(fetch).await.flatten()
}

/// A result, with the index of what it was fetched for.
pub struct Indexed<T> {
    pub index: usize,
    pub value: T,
}

/// Puts the results for the indices `0..n` in the order of their indices.
pub fn in_order<T>(mut results: Vec<Indexed<T>>) -> Vec<T> {
    results.sort_unstable_by_key(|result| result.index);
    results.into_iter().map(|result| result.value).collect()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn results_come_back_in_the_order_of_their_indices() {
        let results = [(2, 'c'), (0, 'a'), (1, 'b')].map(|(index, value)| Indexed { index, value });
        assert_eq!(in_order(results.into()), ['a', 'b', 'c']);
    }
}
