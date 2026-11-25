//! The share of the node limit that the listings of documents may still take.

use std::sync::atomic::{AtomicUsize, Ordering};

/// The number of elements that the listings of one level of a walk may still add.
///
/// Listings run concurrently, so each one reserves its elements before it keeps them.
pub struct ListingBudget(AtomicUsize);

impl ListingBudget {
    pub const fn new(elements: usize) -> Self {
        Self(AtomicUsize::new(elements))
    }

    /// How many elements a listing may add at most.
    pub fn available(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }

    /// Takes `elements` from the budget, until the reservation is dropped without being kept.
    pub fn reserve(&self, elements: usize) -> Option<Reservation<'_>> {
        self.0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| left.checked_sub(elements))
            .ok()?;
        Some(Reservation {
            budget: self,
            elements,
        })
    }
}

/// Elements that a listing took from a [`ListingBudget`]. Dropping the reservation gives them
/// back.
pub struct Reservation<'a> {
    budget: &'a ListingBudget,
    elements: usize,
}

impl Reservation<'_> {
    /// Keeps the elements for good.
    pub fn keep(mut self) {
        self.elements = 0;
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.budget.0.fetch_add(self.elements, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn reservations_share_one_budget() {
        let budget = ListingBudget::new(10);
        let first = budget.reserve(6).expect("6 of 10 fit");
        assert!(budget.reserve(5).is_none(), "5 of the 4 left do not fit");
        drop(first);
        assert_eq!(budget.available(), 10, "a dropped reservation gives its elements back");
        budget.reserve(6).expect("6 of 10 fit").keep();
        assert_eq!(budget.available(), 4, "a kept reservation keeps its elements");
    }
}
