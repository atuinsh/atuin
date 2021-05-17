
Itertools
=========

Extra iterator adaptors, functions and macros.

Please read the `API documentation here`__

__ https://docs.rs/itertools/

|build_status|_ |crates|_

.. |build_status| image:: https://travis-ci.org/rust-itertools/itertools.svg?branch=master
.. _build_status: https://travis-ci.org/rust-itertools/itertools

.. |crates| image:: http://meritbadge.herokuapp.com/itertools
.. _crates: https://crates.io/crates/itertools

How to use with cargo:

.. code:: toml

    [dependencies]
    itertools = "0.9"

How to use in your crate:

.. code:: rust

    use itertools::Itertools;

How to contribute
-----------------

- Fix a bug or implement a new thing
- Include tests for your new feature, preferably a quickcheck test
- Make a Pull Request

For new features, please first consider filing a PR to `rust-lang/rust <https://github.com/rust-lang/rust/>`_,
adding your new feature to the `Iterator` trait of the standard library, if you believe it is reasonable.
If it isn't accepted there, proposing it for inclusion in ``itertools`` is a good idea.
The reason for doing is this is so that we avoid future breakage as with ``.flatten()``.
However, if your feature involves heap allocation, such as storing elements in a ``Vec<T>``,
then it can't be accepted into ``libcore``, and you should propose it for ``itertools`` directly instead.

License
-------

Dual-licensed to be compatible with the Rust project.

Licensed under the Apache License, Version 2.0
http://www.apache.org/licenses/LICENSE-2.0 or the MIT license
http://opensource.org/licenses/MIT, at your
option. This file may not be copied, modified, or distributed
except according to those terms.
