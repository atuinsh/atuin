
This is my substring search workspace.

Please read the `API documentation here`__

__ https://docs.rs/twoway/

|build_status|_ |crates|_

.. |build_status| image:: https://travis-ci.org/bluss/twoway.svg?branch=master
.. _build_status: https://travis-ci.org/bluss/twoway

.. |crates| image:: http://meritbadge.herokuapp.com/twoway
.. _crates: https://crates.io/crates/twoway

Documentation
-------------

Fast substring search for strings and byte strings, using the `two-way algorithm`_.

This is the same code as is included in Rust's libstd to “power” ``str::find(&str)``,
but here it is exposed with some improvements:

- Available for byte string searches using ``&[u8]``
- Having an optional SSE4.2 accelerated version which is even faster.
- Using ``memchr`` for the single byte case, which is ultra fast.

Use cargo feature ``pcmp`` to enable SSE4.2 / pcmpestri accelerated version (only the forward search).

- ``twoway::find_bytes(text: &[u8], pattern: &[u8]) -> Option<usize>``
- ``twoway::rfind_bytes(text: &[u8], pattern: &[u8]) -> Option<usize>``
- ``twoway::find_str(text: &str, pattern: &str) -> Option<usize>``
- ``twoway::rfind_str(text: &str, pattern: &str) -> Option<usize>``

Recent Changes
--------------

- 0.1.8

  - Tweak crate keywords by @tari
  - Only testing and benchmarking changes otherwise (no changes to the crate itself)

- 0.1.7

  - The crate is optionally ``no_std``. Regular and ``pcmp`` both support this
    mode.

- 0.1.6

  - The hidden and internal test module set, technically pub, was removed from
    standard compilation.

- 0.1.5

  - Update from an odds dependency to using ``unchecked-index`` instead
    (only used by the pcmp feature).
  - The hidden and internal test module tw, technically pub, was removed from
    standard compilation.

- 0.1.4

  - Update memchr dependency to 2.0

- 0.1.3

  - Link to docs.rs docs
  - Drop ``pcmp``'s itertools dependency
  - Update nightly code for recent changes

- 0.1.2

  - Internal improvements to the ``pcmp`` module.

- 0.1.1

  - Add ``rfind_bytes``, ``rfind_str``

- 0.1.0

  - Initial release
  - Add ``find_bytes``, ``find_str``

License
-------

MIT / APACHE-2.0


Interesting Links
-----------------

.. _`two-way algorithm`: http://www-igm.univ-mlv.fr/~lecroq/string/node26.html

- Two Way: http://www-igm.univ-mlv.fr/~lecroq/string/node26.html
- Matters Computational: http://www.jjj.de/fxt/#fxtbook


Notes
-----

Consider denying 0/n factorizations, see
http://lists.gnu.org/archive/html/bug-gnulib/2010-06/msg00184.html
