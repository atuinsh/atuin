use std::borrow::Borrow;

use itertools::Itertools;

use crate::slice::{SortedDedupedSliceComparer, partition_dedup};

/// Represents a filter with "or" semantics.
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
#[derive(Clone, Copy, Debug)]
pub struct OrFilter<L> {
    /// The inner list of filters.
    ///
    /// This list must be sorted and contain no duplicates.
    ///
    /// If this list is empty, the filter behaves like "all" (no filtering). If nonempty, the filter
    /// allows items that match any element of the list.
    inner: L,
}

impl<L: FilterStorage> OrFilter<L> {
    /// Create an "all" filter (i.e., allow all items).
    #[must_use]
    pub const fn all() -> Self {
        Self { inner: L::EMPTY }
    }

    /// Create a [`OrFilter`] from a sorted, deduped, non-empty list.
    pub fn new_unchecked(sorted_deduped: L) -> Self {
        debug_assert!(!sorted_deduped.as_ref().is_empty(), "`sorted` cannot be empty");
        debug_assert!(sorted_deduped.as_ref().is_sorted(), "`sorted` must be sorted");
        debug_assert!(
            !sorted_deduped.as_ref().iter().tuple_windows().any(|(a, b)| a == b),
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
    pub fn as_slice_filter(&self) -> OrFilter<&[L::Item]> {
        // The invariants carry over unchanged: the same items in the same order.
        OrFilter {
            inner: self.inner.as_ref(),
        }
    }

    /// Turn this filter into one backed by a [`Vec`].
    ///
    /// # Time complexity
    ///
    /// O(n).
    pub fn to_vec_filter(&self) -> OrFilter<Vec<L::Item>>
    where
        L::Item: Clone,
    {
        // As in `as_slice_filter`, the invariants carry over unchanged.
        OrFilter {
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
    pub fn items(&self) -> Items<&'_ [L::Item]> {
        if self.is_all() {
            Items::All
        } else {
            Items::Some(self.inner.as_ref())
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
        self.inner.as_ref().binary_search_by_key(&item, |item| item.borrow()).is_ok()
    }

    /// Turn this filter into a list of the items in the filter.
    ///
    /// # Time complexity
    ///
    /// O(1).
    pub fn into_list(self) -> Items<L> {
        if self.inner.as_ref().is_empty() {
            Items::All
        } else {
            Items::Some(self.inner)
        }
    }

    /// Compare this filter with an iterator.
    ///
    /// Returns a comparer whose [`eq`](Comparer::eq) method returns true if and only if either of
    /// the following is true:
    ///
    /// * `iter` is [`None`] and this filter is an ["all" filter](Self::is_all).
    /// * The elements of `iter` exactly equal the items in the filter, disregarding order and
    ///   duplicates.
    pub fn compare<'a, I, B>(&'a self, iter: Option<I>) -> Comparer<'a, L::Item, I>
    where
        I: IntoIterator<Item = &'a B>,
        B: Ord + ?Sized + 'a,
        L::Item: Borrow<B>,
    {
        Comparer(match (iter, self.is_all()) {
            (Some(iter), false) => ComparerInner::SliceComparer(SortedDedupedSliceComparer::new(
                self.inner.as_ref(),
                iter,
            )),
            // A concrete list of items can never compare equal to an "all" filter.
            (Some(_), true) => ComparerInner::Immediate(false),
            (None, is_all) => ComparerInner::Immediate(is_all),
        })
    }
}

impl<L: FilterStorageMut> OrFilter<L> {
    /// Create a [`OrFilter`] from a list of items to be included in the filter.
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

impl<L: FilterStorage> Default for OrFilter<L> {
    /// The default value of a [`OrFilter`] is an "all" filter.
    fn default() -> Self {
        Self::all()
    }
}

impl<L, M> PartialEq<OrFilter<M>> for OrFilter<L>
where
    L: FilterStorage,
    M: FilterStorage,
    L::Item: PartialEq<M::Item>,
{
    fn eq(&self, other: &OrFilter<M>) -> bool {
        self.inner.as_ref() == other.inner.as_ref()
    }
}

impl<L> Eq for OrFilter<L>
where
    L: FilterStorage,
    L::Item: Eq,
{
}

/// A borrowed view of the items in a [`OrFilter`].
///
/// `L` can be a slice or [`Vec`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Items<L> {
    /// The filter allows all items.
    All,
    /// The filter allows any item in this slice.
    Some(L),
}

/// A list-like type that can be used with [`OrFilter`].
///
/// This trait is implemented for [`Vec`] and references to slices.
pub trait FilterStorage: Ord + AsRef<[Self::Item]> + sealed::Sealed {
    /// An empty instance of this type. Its `AsRef<[Self::Item]>` implementation must yield an empty
    /// slice.
    const EMPTY: Self;

    type Item: Ord;
}

/// A list-like type that can be used with [`OrFilter`] and mutated.
///
/// This trait is implemented for [`Vec`] and mutable references to slices.
pub trait FilterStorageMut: FilterStorage + AsMut<[Self::Item]> {
    #[must_use]
    fn dedup(self) -> Self;
}

impl<T: Ord> FilterStorage for Vec<T> {
    const EMPTY: Self = Self::new();
    type Item = T;
}

impl<T: Ord> FilterStorageMut for Vec<T> {
    fn dedup(mut self) -> Self {
        Self::dedup(&mut self);
        self
    }
}

impl<T: Ord> FilterStorage for &'_ [T] {
    const EMPTY: Self = &[];
    type Item = T;
}

impl<T: Ord> FilterStorage for &'_ mut [T] {
    // Using a separate function is necessary here -- if `empty_mut_slice()` is replaced with
    // `&mut []`, it won't compile.
    const EMPTY: Self = empty_mut_slice();
    type Item = T;
}

// See comment on `<&mut [T] as FilterStorage>::EMPTY`.
const fn empty_mut_slice<'a, T>() -> &'a mut [T] {
    &mut []
}

impl<T: Ord> FilterStorageMut for &'_ mut [T] {
    fn dedup(self) -> Self {
        partition_dedup(self).0
    }
}

/// Helper type for comparing a [`OrFilter`] with an iterator.
pub struct Comparer<'a, T, I>(ComparerInner<'a, T, I>);

enum ComparerInner<'a, T, I> {
    /// The comparison result is known immediately.
    Immediate(bool),
    /// A [`SortedDedupedSliceComparer`] must be run to obtain the comparison result.
    SliceComparer(SortedDedupedSliceComparer<'a, T, I>),
}

impl<'a, T, I, B> Comparer<'a, T, I>
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
        match self.0 {
            ComparerInner::Immediate(b) => b,
            ComparerInner::SliceComparer(cmp) => cmp.eq::<STACK_SIZE>(),
        }
    }
}

mod sealed {
    pub trait Sealed {}
    impl<T> Sealed for Vec<T> {}
    impl<T> Sealed for &[T] {}
    impl<T> Sealed for &mut [T] {}
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    /// Build a filter from a nonempty list of items.
    fn filter(items: &[&str]) -> OrFilter<Vec<String>> {
        OrFilter::from_list(items.iter().copied().map(str::to_owned).collect::<Vec<_>>())
            .expect("`items` must not be empty")
    }

    #[test]
    fn all_filter_has_no_items() {
        let all = OrFilter::<Vec<String>>::all();
        assert!(all.is_all());
        assert_eq!(all.items(), Items::All);
        assert_eq!(all.into_list(), Items::All);
    }

    #[test]
    fn all_filter_contains_everything() {
        let all = OrFilter::<Vec<String>>::all();
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
        assert_eq!(filter(&["zsh", "bash", "zsh", ""]).items(), Items::Some(expected.as_slice()));
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
        let filter = OrFilter::from_list(items.as_mut_slice()).expect("`items` must not be empty");
        assert_eq!(filter.items(), Items::Some(expected));
    }

    #[test]
    fn from_list_rejects_an_empty_list() {
        assert_eq!(OrFilter::<Vec<String>>::from_list(Vec::new()), None);
    }

    #[test]
    fn from_list_of_none_is_an_all_filter() {
        let filter = OrFilter::<Vec<String>>::from_list(None::<Vec<String>>)
            .expect("`None` yields an \"all\" filter");
        assert!(filter.is_all());
    }

    #[test]
    fn into_list_returns_the_sorted_items() {
        assert_eq!(
            filter(&["zsh", "bash"]).into_list(),
            Items::Some(vec!["bash".to_owned(), "zsh".to_owned()]),
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
    // An "all" filter is not a filter over zero items, so not even an empty list matches it.
    #[case(Some(&[][..]), false)]
    #[case(Some(&["bash"][..]), false)]
    fn compare_with_an_all_filter(#[case] other: Option<&[&str]>, #[case] expected: bool) {
        let all = OrFilter::<Vec<String>>::all();
        let comparer = all.compare(other.map(|o| o.iter().copied()));
        assert_eq!(comparer.eq::<4>(), expected);
    }

    fn any_item() -> impl Strategy<Value = String> {
        // Use a small alphabet to increase chance of repeated items
        "[a-c]{0,2}"
    }

    fn any_items() -> impl Strategy<Value = Vec<String>> {
        prop::collection::vec(any_item(), 0..8)
    }

    fn any_nonempty_items() -> impl Strategy<Value = Vec<String>> {
        prop::collection::vec(any_item(), 1..8)
    }

    fn sort_dedup(items: &[String]) -> Vec<String> {
        items.iter().cloned().collect::<BTreeSet<_>>().into_iter().collect()
    }

    proptest! {
        #[test]
        fn from_list_sorts_dedupes_and_rejects_empty_lists(items in any_items()) {
            let expected = sort_dedup(&items);
            match OrFilter::from_list(items.clone()) {
                None => prop_assert!(items.is_empty()),
                Some(filter) => {
                    prop_assert!(!items.is_empty());
                    prop_assert!(!filter.is_all());
                    prop_assert_eq!(filter.items(), Items::Some(expected.as_slice()));
                }
            }
        }

        #[test]
        fn mut_slice_storage_agrees_with_vec_storage(items in any_nonempty_items()) {
            let mut slice_items = items.clone();
            let from_slice = OrFilter::from_list(slice_items.as_mut_slice())
                .expect("`items` is nonempty");
            let from_vec = OrFilter::from_list(items).expect("`items` is nonempty");
            prop_assert_eq!(from_slice.items(), from_vec.items());
        }

        #[test]
        fn contains_agrees_with_the_item_set(items in any_nonempty_items(), probe in any_item()) {
            let expected = items.contains(&probe);
            let filter = OrFilter::from_list(items).expect("`items` is nonempty");
            prop_assert_eq!(filter.contains(probe.as_str()), expected);
        }

        #[test]
        fn an_all_filter_contains_everything(probe in any_item()) {
            prop_assert!(OrFilter::<Vec<String>>::all().contains(probe.as_str()));
        }

        #[test]
        fn slice_and_vec_views_preserve_the_items(items in any_nonempty_items()) {
            let filter = OrFilter::from_list(items).expect("`items` is nonempty");
            let vec_filter = filter.to_vec_filter();
            let slice_filter = filter.as_slice_filter();
            prop_assert_eq!(slice_filter.items(), filter.items());
            prop_assert_eq!(vec_filter.items(), filter.items());
            prop_assert!(vec_filter == filter);
        }

        #[test]
        fn into_list_returns_the_items(items in any_nonempty_items()) {
            let expected = sort_dedup(&items);
            let filter = OrFilter::from_list(items).expect("`items` is nonempty");
            prop_assert_eq!(filter.into_list(), Items::Some(expected));
        }

        #[test]
        fn equality_is_set_equality(a in any_nonempty_items(), b in any_nonempty_items()) {
            let expected = sort_dedup(&a) == sort_dedup(&b);
            let a = OrFilter::from_list(a).expect("`a` is nonempty");
            let b = OrFilter::from_list(b).expect("`b` is nonempty");
            prop_assert_eq!(a == b, expected);
        }

        #[test]
        fn compare_matches_set_equality(items in any_nonempty_items(), other in any_items()) {
            let expected = sort_dedup(&items) == sort_dedup(&other);
            let filter = OrFilter::from_list(items).expect("`items` is nonempty");

            // All values of `STACK_SIZE` must give the same result.
            prop_assert_eq!(filter.compare(Some(other.iter().map(String::as_str))).eq::<4>(), expected);
            prop_assert_eq!(filter.compare(Some(other.iter().map(String::as_str))).eq::<0>(), expected);
            prop_assert_eq!(filter.compare(Some(other.iter().map(String::as_str))).eq::<64>(), expected);

            let reversed = other.iter().rev().map(String::as_str);
            prop_assert_eq!(filter.compare(Some(reversed)).eq::<4>(), expected);
            let duplicated = other.iter().chain(other.iter()).map(String::as_str);
            prop_assert_eq!(filter.compare(Some(duplicated)).eq::<4>(), expected);

            // A non-"all" filter never compares equal to `None`.
            prop_assert!(!filter.compare(None::<std::iter::Empty<&str>>).eq::<4>());
        }

        #[test]
        fn an_all_filter_compares_equal_only_to_none(other in any_items()) {
            let all = OrFilter::<Vec<String>>::all();
            prop_assert!(all.compare(None::<std::iter::Empty<&str>>).eq::<4>());
            prop_assert!(!all.compare(Some(other.iter().map(String::as_str))).eq::<4>());
            prop_assert!(!all.compare(Some(other.iter().map(String::as_str))).eq::<0>());
            prop_assert!(!all.compare(Some(other.iter().map(String::as_str))).eq::<64>());
        }
    }
}
