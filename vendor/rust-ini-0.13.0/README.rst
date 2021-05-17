INI in Rust
-----------

.. image:: https://travis-ci.org/zonyitoo/rust-ini.svg?branch=master
    :target: https://travis-ci.org/zonyitoo/rust-ini

.. image:: https://img.shields.io/crates/v/rust-ini.svg
    :target: https://crates.io/crates/rust-ini

INI_ is an informal standard for configuration files for some platforms or software. INI files are simple text files with a basic structure composed of "sections" and "properties".

.. _INI: http://en.wikipedia.org/wiki/INI_file

This is an INI file parser in Rust_.

.. _Rust: http://www.rust-lang.org/

.. code:: toml

    [dependencies]
    rust-ini = "0.13"

Usage
=====

* Create a Ini configuration file.

.. code:: rust

    extern crate ini;
    use ini::Ini;

    fn main() {
        let mut conf = Ini::new();
        conf.with_section(None)
            .set("encoding", "utf-8");
        conf.with_section(Some("User".to_owned()))
            .set("given_name", "Tommy")
            .set("family_name", "Green")
            .set("unicode", "Raspberry树莓");
        conf.with_section(Some("Book".to_owned()))
            .set("name", "Rust cool");
        conf.write_to_file("conf.ini").unwrap();
    }

Then you will get ``conf.ini``

.. code:: ini

    encoding=utf-8

    [User]
    given_name=Tommy
    family_name=Green
    unicode=Raspberry\x6811\x8393

    [Book]
    name=Rust cool

* Read from file ``conf.ini``

.. code:: rust

    extern crate ini;
    use ini::Ini;

    fn main() {
        let conf = Ini::load_from_file("conf.ini").unwrap();

        let section = conf.section(Some("User".to_owned())).unwrap();
        let tommy = section.get("given_name").unwrap();
        let green = section.get("family_name").unwrap();

        println!("{:?} {:?}", tommy, green);

        // iterating
        for (sec, prop) in &conf {
            println!("Section: {:?}", sec);
            for (key, value) in &prop {
                println!("{:?}:{:?}", key, value);
            }
        }
    }

* More details could be found in `examples`.

License
=======

`The MIT License (MIT)`_

.. _The MIT License (MIT): https://opensource.org/licenses/MIT

Copyright (c) 2014 Y. T. CHUNG

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
