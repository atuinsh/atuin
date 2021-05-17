# 0.4.3 (February 6, 2021)

* Add no_std support for Rust 1.36 and above (#71).
* Add `get2_mut` and `get2_unchecked_mut` methods (#65).
* Make `shrink_to_fit()` remove trailing vacant entries (#62).
* Implement `FromIterator<(usize, T)>` (#62).
* Implement `IntoIterator<Item = (usize, T)>` (#62).
* Provide `size_hint()` of the iterators (#62).
* Make all iterators reversible (#62).
* Add `key_of()` method (#61)
* Add `compact()` method (#60)
* Add support for serde (#85)

# 0.4.2 (January 11, 2019)

* Add `Slab::drain` (#56).

# 0.4.1 (July 15, 2018)

* Improve `reserve` and `reserve_exact` (#37).
* Implement `Default` for `Slab` (#43).
