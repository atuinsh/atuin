#![allow(unsafe_code)]
use crate::utils::SortedDedupedSliceComparer;
use itertools::Itertools;
use std::borrow::Borrow;

/// Represents a filter with disjunctive semantics.
///
/// The filter contains one or more items. An item is allowed through the filter if and only if it
/// matches one of the items in the filter. In essence, the filter behaves like a giant "or"
/// expression.
///
/// The filter can also be an "all" filter, which allows all items unconditionally. This type's
/// implementation of [`Default`] returns an "all" filter.
///
/// `L` is one of the following list types:
///
/// * [`Vec<T>`]
/// * [`&[T]`](slice)
/// * [`&mut [T]`](slice)
#[derive(Clone, Copy, Debug, Default)]
pub struct DisjunctiveFilter<L> {
    /// The inner list of filters.
    ///
    /// This list must be sorted and contain no duplicates.
    ///
    /// If this list is empty, the filter behaves like "all" (no filtering). If nonempty, the filter
    /// allows items that match any element of the list.
    inner: L,
}

impl<L: FilterStorage> DisjunctiveFilter<L> {
    /// Create an "all" filter (i.e., allow all items).
    pub const fn all() -> Self {
        Self { inner: L::DEFAULT }
    }

    /// Create a [`DisjunctiveFilter`] from a sorted, deduped, non-empty list.
    pub fn new_unchecked(sorted_deduped: L) -> Self {
        debug_assert!(
            !sorted_deduped.as_ref().is_empty(),
            "`sorted` cannot be empty"
        );
        debug_assert!(
            sorted_deduped.as_ref().is_sorted(),
            "`sorted` must be sorted"
        );
        debug_assert!(
            !sorted_deduped
                .as_ref()
                .iter()
                .tuple_windows()
                .any(|(a, b)| a == b),
            "`sorted` must not contain duplicates",
        );
        Self {
            inner: sorted_deduped,
        }
    }

    /// View this filter as one backed by a slice.
    ///
    /// The returned filter is [`Copy`], so it can be passed around freely without allocating or
    /// keeping a reference to `self` alive as a whole.
    ///
    /// # Time complexity
    ///
    /// O(1).
    pub fn as_slice_filter(&self) -> DisjunctiveFilter<&[L::Item]> {
        // The invariants carry over unchanged: the same items in the same order.
        DisjunctiveFilter {
            inner: self.inner.as_ref(),
        }
    }

    /// Turn this filter into one backed by a [`Vec`].
    ///
    /// # Time complexity
    ///
    /// O(n).
    pub fn to_vec_filter(&self) -> DisjunctiveFilter<Vec<L::Item>>
    where
        L::Item: Clone,
    {
        // As in `as_slice_filter`, the invariants carry over unchanged.
        DisjunctiveFilter {
            inner: self.inner.as_ref().to_vec(),
        }
    }

    /// Check whether this filter is an "all" filter (i.e., allows all items).
    pub fn is_all(&self) -> bool {
        self.inner.as_ref().is_empty()
    }

    /// Get the set of items this filter permits.
    ///
    /// The items are sorted and contain no duplicates.
    ///
    /// Returns [`None`] if the filter allows all items.
    pub fn items(&self) -> Option<&[L::Item]> {
        if self.is_all() {
            None
        } else {
            Some(self.inner.as_ref())
        }
    }

    /// Check whether this filter contains a particular item.
    ///
    /// If the filter is an ["all" filter](Self::is_all), this will always return true.
    ///
    /// # Time complexity
    ///
    /// Worst-case O(log n).
    pub fn contains<U>(&self, item: &U) -> bool
    where
        U: Ord + ?Sized,
        L::Item: Borrow<U>,
    {
        if self.is_all() {
            return true;
        }
        self.inner
            .as_ref()
            .binary_search_by_key(&item, |item| item.borrow())
            .is_ok()
    }

    /// Turn this filter into a list of the items in the filter.
    ///
    /// Returns [`None`] if this filter is an ["all" filter](Self::is_all).
    ///
    /// # Time complexity
    ///
    /// O(1).
    pub fn into_list(self) -> Option<L> {
        if self.inner.as_ref().is_empty() {
            None
        } else {
            Some(self.inner)
        }
    }

    /// Compare this filter with an iterator.
    ///
    /// Returns a comparer whose [`eq`](DisjunctiveFilterComparer::eq) method returns true if and
    /// only if either of the following is true:
    ///
    /// * `iter` is [`None`] and this filter is an ["all" filter](Self::is_all).
    /// * The elements of `iter` exactly equal the items in the filter, disregarding order and
    ///   duplicates.
    pub fn compare<'a, I, B>(&'a self, iter: Option<I>) -> DisjunctiveFilterComparer<'a, L::Item, I>
    where
        I: IntoIterator<Item = &'a B>,
        B: Ord + ?Sized + 'a,
        L::Item: Borrow<B>,
    {
        DisjunctiveFilterComparer {
            comparer: iter.map(|iter| SortedDedupedSliceComparer::new(self.inner.as_ref(), iter)),
            is_all: self.is_all(),
        }
    }
}

impl<L: FilterStorageMut> DisjunctiveFilter<L> {
    /// Create a [`DisjunctiveFilter`] from a list of items to be included in the filter.
    ///
    /// `list` can be an `Option<L>` or `L`. In the case of [`Option`], [`None`] means no filtering
    /// (i.e., an "all" filter), in which case [`Self::all()`] will be returned.
    ///
    /// If an empty list is provided, this function will return [`None`], because this type cannot
    /// represent empty filter lists.
    ///
    /// # Time complexity
    ///
    /// Worst-case O(n log n), but if `list` is already sorted, worst-case O(n).
    pub fn from_list<M>(list: M) -> Option<Self>
    where
        M: Into<Option<L>>,
    {
        let Some(mut list) = list.into() else {
            return Some(Self::all());
        };
        if list.as_ref().is_empty() {
            return None;
        }
        list.as_mut().sort_unstable();
        Some(Self::new_unchecked(list.dedup()))
    }
}

impl<L, M> PartialEq<DisjunctiveFilter<M>> for DisjunctiveFilter<L>
where
    L: FilterStorage,
    M: FilterStorage,
    L::Item: PartialEq<M::Item>,
{
    fn eq(&self, other: &DisjunctiveFilter<M>) -> bool {
        self.inner.as_ref() == other.inner.as_ref()
    }
}

/// A list-like type that can be used with [`DisjunctiveFilter`].
///
/// This trait is implemented for [`Vec`] and references to slices.
pub trait FilterStorage: Ord + Default + AsRef<[Self::Item]> {
    /// We need to be able to obtain a default value in const contexts, so we can't just rely on the
    /// [`Default`] trait; we need an associated const too.
    const DEFAULT: Self;

    type Item: Ord;
}

/// A list-like type that can be used with [`DisjunctiveFilter`] and mutated.
///
/// This trait is implemented for [`Vec`] and mutable references to slices.
pub trait FilterStorageMut: FilterStorage + AsMut<[Self::Item]> {
    fn dedup(self) -> Self;
}

impl<T: Ord> FilterStorage for Vec<T> {
    const DEFAULT: Self = Self::new();
    type Item = T;
}

impl<T: Ord> FilterStorageMut for Vec<T> {
    fn dedup(mut self) -> Self {
        Vec::dedup(&mut self);
        self
    }
}

impl<T: Ord> FilterStorage for &'_ [T] {
    const DEFAULT: Self = &[];
    type Item = T;
}

impl<T: Ord> FilterStorage for &'_ mut [T] {
    // Using a separate function is necessary here -- if `empty_mut_slice()` is replaced with
    // `&mut []`, it won't compile.
    const DEFAULT: Self = empty_mut_slice();
    type Item = T;
}

// See comment on `<&mut [T] as FilterStorage>::DEFAULT`.
const fn empty_mut_slice<'a, T>() -> &'a mut [T] {
    &mut []
}

impl<T: Ord> FilterStorageMut for &'_ mut [T] {
    fn dedup(self) -> Self {
        // TODO: this is the same algorithm as `slice::partition_dedup`, which is currently
        // unstable. Use that method instead once it's stabilized.
        if self.len() <= 1 {
            return self;
        }

        let mut next_read: usize = 1;
        let mut next_write: usize = 1;

        while next_read < self.len() {
            if self[next_read] != self[next_write - 1] {
                self.swap(next_read, next_write);
                next_write += 1;
            }
            next_read += 1;
        }
        &mut self[..next_write]
    }
}

/// Helper type for comparing a [`DisjunctiveFilter`] with an iterator.
pub struct DisjunctiveFilterComparer<'a, T, I> {
    /// [`None`] if no iterator was provided.
    comparer: Option<SortedDedupedSliceComparer<'a, T, I>>,
    /// Whether the filter is an ["all" filter](DisjunctiveFilter::is_all).
    is_all: bool,
}

impl<'a, T, I, B> DisjunctiveFilterComparer<'a, T, I>
where
    I: IntoIterator<Item = &'a B>,
    B: Ord + ?Sized + 'a,
    T: Borrow<B>,
{
    /// Check whether the elements of the iterator are exactly equal to the items in the filter, or
    /// whether no iterator was provided and this is an "all" filter.
    ///
    /// If the number of items in the filter is less than or equal to `STACK_SIZE`, this method will
    /// not allocate memory.
    pub fn eq<const STACK_SIZE: usize>(self) -> bool {
        match self.comparer {
            Some(comparer) => comparer.eq::<STACK_SIZE>(),
            None => self.is_all,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DisjunctiveFilter;
    use rstest::rstest;

    /// Build a filter from a nonempty list of items.
    fn filter(items: &[&str]) -> DisjunctiveFilter<Vec<String>> {
        DisjunctiveFilter::from_list(items.iter().copied().map(str::to_owned).collect::<Vec<_>>())
            .expect("`items` must not be empty")
    }

    #[test]
    fn all_filter_has_no_items() {
        let all = DisjunctiveFilter::<Vec<String>>::all();
        assert!(all.is_all());
        assert_eq!(all.items(), None);
        assert_eq!(all.into_list(), None);
    }

    #[test]
    fn all_filter_contains_everything() {
        let all = DisjunctiveFilter::<Vec<String>>::all();
        assert!(all.contains("bash"));
        assert!(all.contains(""));
        assert!(all.contains("anything at all"));
    }

    #[rstest]
    #[case(&["bash"], "bash", true)]
    #[case(&["bash"], "zsh", false)]
    #[case(&["", "bash"], "", true)]
    #[case(&["", "bash"], "zsh", false)]
    #[case(&["bash", "fish", "zsh"], "fish", true)]
    #[case(&["bash", "fish", "zsh"], "nu", false)]
    fn contains_only_listed_items(
        #[case] items: &[&str],
        #[case] probe: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(filter(items).contains(probe), expected);
    }

    #[test]
    fn from_list_sorts_and_dedupes() {
        let expected = ["", "bash", "zsh"].map(str::to_owned);
        assert_eq!(
            filter(&["zsh", "bash", "zsh", ""]).items(),
            Some(expected.as_slice())
        );
    }

    #[rstest]
    #[case(&["zsh", "bash", "zsh", ""], &["", "bash", "zsh"])]
    #[case(&["a", "a", "a"], &["a"])]
    #[case(&["b", "a", "a"], &["a", "b"])]
    #[case(&["a", "b", "b"], &["a", "b"])]
    #[case(&["a"], &["a"])]
    fn from_list_of_a_mut_slice_sorts_and_dedupes(
        #[case] items: &[&str],
        #[case] expected: &[&str],
    ) {
        let mut items = items.to_vec();
        let filter =
            DisjunctiveFilter::from_list(items.as_mut_slice()).expect("`items` must not be empty");
        assert_eq!(filter.items(), Some(expected));
    }

    #[test]
    fn from_list_rejects_an_empty_list() {
        assert_eq!(
            DisjunctiveFilter::<Vec<String>>::from_list(Vec::new()),
            None
        );
    }

    #[test]
    fn from_list_of_none_is_an_all_filter() {
        let filter = DisjunctiveFilter::<Vec<String>>::from_list(None::<Vec<String>>)
            .expect("`None` yields an \"all\" filter");
        assert!(filter.is_all());
    }

    #[test]
    fn into_list_returns_the_sorted_items() {
        assert_eq!(
            filter(&["zsh", "bash"]).into_list(),
            Some(vec!["bash".to_owned(), "zsh".to_owned()]),
        );
    }

    #[rstest]
    #[case(&["bash"], Some(&["bash"][..]), true)]
    #[case(&["bash"], Some(&["bash", "bash"][..]), true)]
    #[case(&["bash", "zsh"], Some(&["zsh", "bash"][..]), true)]
    #[case(&["bash", "zsh"], Some(&["bash"][..]), false)]
    #[case(&["bash"], Some(&["bash", "zsh"][..]), false)]
    #[case(&["bash"], Some(&[][..]), false)]
    #[case(&["bash"], None, false)]
    fn compare_with_a_non_all_filter(
        #[case] items: &[&str],
        #[case] other: Option<&[&str]>,
        #[case] expected: bool,
    ) {
        // Bind the filter: the comparer borrows it, so it must outlive the statement.
        let f = filter(items);
        let comparer = f.compare(other.map(|o| o.iter().copied()));
        assert_eq!(comparer.eq::<4>(), expected);
    }

    #[rstest]
    #[case(None, true)]
    #[case(Some(&[][..]), true)]
    #[case(Some(&["bash"][..]), false)]
    fn compare_with_an_all_filter(#[case] other: Option<&[&str]>, #[case] expected: bool) {
        let all = DisjunctiveFilter::<Vec<String>>::all();
        let comparer = all.compare(other.map(|o| o.iter().copied()));
        assert_eq!(comparer.eq::<4>(), expected);
    }
}
