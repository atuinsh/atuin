indexmap
========

|build_status|_ |crates|_ |docs|_ |rustc|_

.. |build_status| image:: https://github.com/bluss/indexmap/workflows/Continuous%20integration/badge.svg?branch=master
.. _build_status: https://github.com/bluss/indexmap/actions

.. |crates| image:: https://img.shields.io/crates/v/indexmap.svg
.. _crates: https://crates.io/crates/indexmap

.. |docs| image:: https://docs.rs/indexmap/badge.svg
.. _docs: https://docs.rs/indexmap

.. |rustc| image:: https://img.shields.io/badge/rust-1.36%2B-orange.svg
.. _rustc: https://img.shields.io/badge/rust-1.36%2B-orange.svg

A pure-Rust hash table which preserves (in a limited sense) insertion order.

This crate implements compact map and set data-structures,
where the iteration order of the keys is independent from their hash or
value. It preserves insertion order (except after removals), and it
allows lookup of entries by either hash table key or numerical index.

Note: this crate was originally released under the name ``ordermap``,
but it was renamed to ``indexmap`` to better reflect its features.

Background
==========

This was inspired by Python 3.6's new dict implementation (which remembers
the insertion order and is fast to iterate, and is compact in memory).

Some of those features were translated to Rust, and some were not. The result
was indexmap, a hash table that has following properties:

- Order is **independent of hash function** and hash values of keys.
- Fast to iterate.
- Indexed in compact space.
- Preserves insertion order **as long** as you don't call ``.remove()``.
- Uses hashbrown for the inner table, just like Rust's libstd ``HashMap`` does.

Performance
-----------

``IndexMap`` derives a couple of performance facts directly from how it is constructed,
which is roughly:

  A raw hash table of key-value indices, and a vector of key-value pairs.

- Iteration is very fast since it is on the dense key-values.
- Removal is fast since it moves memory areas only in the table,
  and uses a single swap in the vector.
- Lookup is fast-ish because the initial 7-bit hash lookup uses SIMD, and indices are
  densely stored. Lookup also is slow-ish since the actual key-value pairs are stored
  separately. (Visible when cpu caches size is limiting.)

- In practice, ``IndexMap`` has been tested out as the hashmap in rustc in PR45282_ and
  the performance was roughly on par across the whole workload. 
- If you want the properties of ``IndexMap``, or its strongest performance points
  fits your workload, it might be the best hash table implementation.

.. _PR45282: https://github.com/rust-lang/rust/pull/45282


Recent Changes
==============

- 1.6.2

  - Fixed to match ``std`` behavior, ``OccupiedEntry::key`` now references the
    existing key in the map instead of the lookup key, by @cuviper in PR 170_.

  - The new ``Entry::or_insert_with_key`` matches Rust 1.50's ``Entry`` method,
    passing ``&K`` to the callback to create a value, by @cuviper in PR 175_.

.. _170: https://github.com/bluss/indexmap/pull/170
.. _175: https://github.com/bluss/indexmap/pull/175

- 1.6.1

  - The new ``serde_seq`` module implements ``IndexMap`` serialization as a
    sequence to ensure order is preserved, by @cuviper in PR 158_.

  - New methods on maps and sets work like the ``Vec``/slice methods by the same name:
    ``truncate``, ``split_off``, ``first``, ``first_mut``, ``last``, ``last_mut``, and
    ``swap_indices``, by @cuviper in PR 160_.

.. _158: https://github.com/bluss/indexmap/pull/158
.. _160: https://github.com/bluss/indexmap/pull/160

- 1.6.0

  - **MSRV**: Rust 1.36 or later is now required.

  - The ``hashbrown`` dependency has been updated to version 0.9.

- 1.5.2

  - The new "std" feature will force the use of ``std`` for users that explicitly
    want the default ``S = RandomState``, bypassing the autodetection added in 1.3.0,
    by @cuviper in PR 145_.

.. _145: https://github.com/bluss/indexmap/pull/145

- 1.5.1

  - Values can now be indexed by their ``usize`` position by @cuviper in PR 132_.

  - Some of the generic bounds have been relaxed to match ``std`` by @cuviper in PR 141_.

  - ``drain`` now accepts any ``R: RangeBounds<usize>`` by @cuviper in PR 142_.

.. _132: https://github.com/bluss/indexmap/pull/132
.. _141: https://github.com/bluss/indexmap/pull/141
.. _142: https://github.com/bluss/indexmap/pull/142

- 1.5.0

  - **MSRV**: Rust 1.32 or later is now required.

  - The inner hash table is now based on ``hashbrown`` by @cuviper in PR 131_.
    This also completes the method ``reserve`` and adds ``shrink_to_fit``.

  - Add new methods ``get_key_value``, ``remove_entry``, ``swap_remove_entry``,
    and ``shift_remove_entry``, by @cuviper in PR 136_

  - ``Clone::clone_from`` reuses allocations by @cuviper in PR 125_

  - Add new method ``reverse`` by @linclelinkpart5 in PR 128_

.. _125: https://github.com/bluss/indexmap/pull/125
.. _128: https://github.com/bluss/indexmap/pull/128
.. _131: https://github.com/bluss/indexmap/pull/131
.. _136: https://github.com/bluss/indexmap/pull/136

- 1.4.0

  - Add new method ``get_index_of`` by @Thermatrix in PR 115_ and 120_

  - Fix build script rebuild-if-changed configuration to use "build.rs";
    fixes issue 123_. Fix by @cuviper.

  - Dev-dependencies (rand and quickcheck) have been updated. The crate's tests
    now run using Rust 1.32 or later (MSRV for building the crate has not changed).
    by @kjeremy and @bluss

.. _123: https://github.com/bluss/indexmap/issues/123
.. _115: https://github.com/bluss/indexmap/pull/115
.. _120: https://github.com/bluss/indexmap/pull/120

- 1.3.2

  - Maintenance update to regenerate the published `Cargo.toml`.

- 1.3.1

  - Maintenance update for formatting and ``autocfg`` 1.0.

- 1.3.0

  - The deprecation messages in the previous version have been removed.
    (The methods have not otherwise changed.) Docs for removal methods have been
    improved.
  - From Rust 1.36, this crate supports being built **without std**, requiring
    ``alloc`` instead. This is enabled automatically when it is detected that
    ``std`` is not available. There is no crate feature to enable/disable to
    trigger this. The new build-dep ``autocfg`` enables this.

- 1.2.0

  - Plain ``.remove()`` now has a deprecation message, it informs the user
    about picking one of the removal functions ``swap_remove`` and ``shift_remove``
    which have different performance and order semantics.
    Plain ``.remove()`` will not be removed, the warning message and method
    will remain until further.

  - Add new method ``shift_remove`` for order preserving removal on the map,
    and ``shift_take`` for the corresponding operation on the set.

  - Add methods ``swap_remove``, ``swap_remove_entry`` to ``Entry``.

  - Fix indexset/indexmap to support full paths, like ``indexmap::indexmap!()``

  - Internal improvements: fix warnings, deprecations and style lints

- 1.1.0

  - Added optional feature `"rayon"` that adds parallel iterator support
    to `IndexMap` and `IndexSet` using Rayon. This includes all the regular
    iterators in parallel versions, and parallel sort.

  - Implemented ``Clone`` for ``map::{Iter, Keys, Values}`` and
    ``set::{Difference, Intersection, Iter, SymmetricDifference, Union}``

  - Implemented ``Debug`` for ``map::{Entry, IntoIter, Iter, Keys, Values}`` and
    ``set::{Difference, Intersection, IntoIter, Iter, SymmetricDifference, Union}``

  - Serde trait ``IntoDeserializer`` are implemented for ``IndexMap`` and ``IndexSet``.

  - Minimum Rust version requirement increased to Rust 1.30 for development builds.

- 1.0.2

  - The new methods ``IndexMap::insert_full`` and ``IndexSet::insert_full`` are
    both like ``insert`` with the index included in the return value.

  - The new method ``Entry::and_modify`` can be used to modify occupied
    entries, matching the new methods of ``std`` maps in Rust 1.26.

  - The new method ``Entry::or_default`` inserts a default value in unoccupied
    entries, matching the new methods of ``std`` maps in Rust 1.28.

- 1.0.1

  - Document Rust version policy for the crate (see rustdoc)

- 1.0.0

  - This is the 1.0 release for ``indexmap``! (the crate and datastructure
    formerly known as “ordermap”)
  - ``OccupiedEntry::insert`` changed its signature, to use ``&mut self`` for
    the method receiver, matching the equivalent method for a standard
    ``HashMap``.  Thanks to @dtolnay for finding this bug.
  - The deprecated old names from ordermap were removed: ``OrderMap``,
    ``OrderSet``, ``ordermap!{}``, ``orderset!{}``. Use the new ``IndexMap``
    etc names instead.

- 0.4.1

  - Renamed crate to ``indexmap``; the ``ordermap`` crate is now deprecated
    and the types ``OrderMap/Set`` now have a deprecation notice.

- 0.4.0

  - This is the last release series for this ``ordermap`` under that name,
    because the crate is **going to be renamed** to ``indexmap`` (with types
    ``IndexMap``, ``IndexSet``) and no change in functionality!
  - The map and its associated structs moved into the ``map`` submodule of the
    crate, so that the map and set are symmetric

    + The iterators, ``Entry`` and other structs are now under ``ordermap::map::``

  - Internally refactored ``OrderMap<K, V, S>`` so that all the main algorithms
    (insertion, lookup, removal etc) that don't use the ``S`` parameter (the
    hasher) are compiled without depending on ``S``, which reduces generics bloat.

  - ``Entry<K, V>`` no longer has a type parameter ``S``, which is just like
    the standard ``HashMap``'s entry.

  - Minimum Rust version requirement increased to Rust 1.18

- 0.3.5

  - Documentation improvements

- 0.3.4

  - The ``.retain()`` methods for ``OrderMap`` and ``OrderSet`` now
    traverse the elements in order, and the retained elements **keep their order**
  - Added new methods ``.sort_by()``, ``.sort_keys()`` to ``OrderMap`` and
    ``.sort_by()``, ``.sort()`` to ``OrderSet``. These methods allow you to
    sort the maps in place efficiently.

- 0.3.3

  - Document insertion behaviour better by @lucab
  - Updated dependences (no feature changes) by @ignatenkobrain

- 0.3.2

  - Add ``OrderSet`` by @cuviper!
  - ``OrderMap::drain`` is now (too) a double ended iterator.

- 0.3.1

  - In all ordermap iterators, forward the ``collect`` method to the underlying
    iterator as well.
  - Add crates.io categories.

- 0.3.0

  - The methods ``get_pair``, ``get_pair_index`` were both replaced by
    ``get_full`` (and the same for the mutable case).
  - Method ``swap_remove_pair`` replaced by ``swap_remove_full``.
  - Add trait ``MutableKeys`` for opt-in mutable key access. Mutable key access
    is only possible through the methods of this extension trait.
  - Add new trait ``Equivalent`` for key equivalence. This extends the
    ``Borrow`` trait mechanism for ``OrderMap::get`` in a backwards compatible
    way, just some minor type inference related issues may become apparent.
    See `#10`__ for more information.
  - Implement ``Extend<(&K, &V)>`` by @xfix.

__ https://github.com/bluss/ordermap/pull/10

- 0.2.13

  - Fix deserialization to support custom hashers by @Techcable.
  - Add methods ``.index()`` on the entry types by @garro95.

- 0.2.12

  - Add methods ``.with_hasher()``, ``.hasher()``.

- 0.2.11

  - Support ``ExactSizeIterator`` for the iterators. By @Binero.
  - Use ``Box<[Pos]>`` internally, saving a word in the ``OrderMap`` struct.
  - Serde support, with crate feature ``"serde-1"``. By @xfix.

- 0.2.10

  - Add iterator ``.drain(..)`` by @stevej.

- 0.2.9

  - Add method ``.is_empty()`` by @overvenus.
  - Implement ``PartialEq, Eq`` by @overvenus.
  - Add method ``.sorted_by()``.

- 0.2.8

  - Add iterators ``.values()`` and ``.values_mut()``.
  - Fix compatibility with 32-bit platforms.

- 0.2.7

  - Add ``.retain()``.

- 0.2.6

  - Add ``OccupiedEntry::remove_entry`` and other minor entry methods,
    so that it now has all the features of ``HashMap``'s entries.

- 0.2.5

  - Improved ``.pop()`` slightly.

- 0.2.4

  - Improved performance of ``.insert()`` (`#3`__) by @pczarn.

__ https://github.com/bluss/ordermap/pull/3

- 0.2.3

  - Generalize ``Entry`` for now, so that it works on hashmaps with non-default
    hasher. However, there's a lingering compat issue since libstd ``HashMap``
    does not parameterize its entries by the hasher (``S`` typarm).
  - Special case some iterator methods like ``.nth()``.

- 0.2.2

  - Disable the verbose ``Debug`` impl by default.

- 0.2.1

  - Fix doc links and clarify docs.

- 0.2.0

  - Add more ``HashMap`` methods & compat with its API.
  - Experimental support for ``.entry()`` (the simplest parts of the API).
  - Add ``.reserve()`` (placeholder impl).
  - Add ``.remove()`` as synonym for ``.swap_remove()``.
  - Changed ``.insert()`` to swap value if the entry already exists, and
    return ``Option``.
  - Experimental support as an *indexed* hash map! Added methods
    ``.get_index()``, ``.get_index_mut()``, ``.swap_remove_index()``,
    ``.get_pair_index()``, ``.get_pair_index_mut()``.

- 0.1.2

  - Implement the 32/32 split idea for ``Pos`` which improves cache utilization
    and lookup performance.

- 0.1.1

  - Initial release.
