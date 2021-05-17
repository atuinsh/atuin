# Change Log
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/)
and this project adheres to [Semantic Versioning](http://semver.org/).

## 0.11.0 - 2021-03-17
 - The `Config` type got a builder-pattern `with_merged()` method [#166].
 - A `Config::set_once()` function was added, to set an value that can be
   overwritten by `Config::merge`ing another configuration [#172]
 - serde_hjson is, if enabled, pulled in without default features.
   This is due to a bug in serde_hjson, see [#169] for more information.
 - Testing is done on github actions [#175]

[#166]: https://github.com/mehcode/config-rs/pull/166
[#172]: https://github.com/mehcode/config-rs/pull/172
[#169]: https://github.com/mehcode/config-rs/pull/169
[#175]: https://github.com/mehcode/config-rs/pull/169

## 0.10.1 - 2019-12-07
 - Allow enums as configuration keys [#119]

[#119]: https://github.com/mehcode/config-rs/pull/119

## 0.10.0 - 2019-12-07
 - Remove lowercasing of keys (unless the key is coming from an environment variable).
 - Update nom to 5.x

## 0.9.3 - 2019-05-09
 - Support deserializing to a struct with `#[serde(default)]` [#106]

[#106]: https://github.com/mehcode/config-rs/pull/106

## 0.9.2 - 2019-01-03
 - Support reading `enum`s from configuration. [#85]
 - Improvements to error path (attempting to propagate path). [#89]
 - Fix UB in monomorphic expansion. We weren't re-exporting dependent types. [#91]

[#85]: https://github.com/mehcode/config-rs/pull/85
[#89]: https://github.com/mehcode/config-rs/pull/89
[#91]: https://github.com/mehcode/config-rs/issues/91

## 0.9.1 - 2018-09-25
 - Allow Environment variable collection to ignore empty values. [#78]
   ```rust
   // Empty env variables will not be collected
   Environment::with_prefix("APP").ignore_empty(true)
   ```

[#78]: https://github.com/mehcode/config-rs/pull/78

## 0.9.0 - 2018-07-02
 - **Breaking Change:** Environment does not declare a separator by default.
    ```rust
    // 0.8.0
    Environment::with_prefix("APP")

    // 0.9.0
    Environment::with_prefix("APP").separator("_")
    ```

 - Add support for INI. [#72]
 - Add support for newtype structs. [#71]
 - Fix bug with array set by path. [#69]
 - Update to nom 4. [#63]

[#72]: https://github.com/mehcode/config-rs/pull/72
[#71]: https://github.com/mehcode/config-rs/pull/71
[#69]: https://github.com/mehcode/config-rs/pull/69
[#63]: https://github.com/mehcode/config-rs/pull/63

## 0.8.0 - 2018-01-26
 - Update lazy_static and yaml_rust

## 0.7.1 - 2018-01-26
 - Be compatible with nom's verbose_errors feature (#50)[https://github.com/mehcode/config-rs/pull/50]
 - Add `derive(PartialEq)` for Value (#54)[https://github.com/mehcode/config-rs/pull/54]

## 0.7.0 - 2017-08-05
 - Fix conflict with `serde_yaml`. [#39]

[#39]: https://github.com/mehcode/config-rs/issues/39

 - Implement `Source` for `Config`.
 - Implement `serde::de::Deserializer` for `Config`. `my_config.deserialize` may now be called as either `Deserialize::deserialize(my_config)` or `my_config.try_into()`.
 - Remove `ConfigResult`. The builder pattern requires either `.try_into` as the final step _or_ the initial `Config::new()` to be bound to a slot. Errors must also be handled on each call instead of at the end of the chain.


    ```rust
    let mut c = Config::new();
    c
        .merge(File::with_name("Settings")).unwrap()
        .merge(Environment::with_prefix("APP")).unwrap();
    ```

    ```rust
    let c = Config::new()
        .merge(File::with_name("Settings")).unwrap()
        .merge(Environment::with_prefix("APP")).unwrap()
        // LLVM should be smart enough to remove the actual clone operation
        // as you are cloning a temporary that is dropped at the same time
        .clone();
    ```

    ```rust
    let mut s: Settings = Config::new()
        .merge(File::with_name("Settings")).unwrap()
        .merge(Environment::with_prefix("APP")).unwrap()
        .try_into();
    ```

## 0.6.0 – 2017-06-22
  - Implement `Source` for `Vec<T: Source>` and `Vec<Box<Source>>`

    ```rust
    Config::new()
        .merge(vec![
            File::with_name("config/default"),
            File::with_name(&format!("config/{}", run_mode)),
        ])
    ```

  - Implement `From<&Path>` and `From<PathBuf>` for `File`

  - Remove `namespace` option for File
  - Add builder pattern to condense configuration

    ```rust
    Config::new()
        .merge(File::with_name("Settings"))
        .merge(Environment::with_prefix("APP"))
        .unwrap()
    ```

 - Parsing errors even for non required files – [@Anthony25] ( [#33] )

[@Anthony25]: https://github.com/Anthony25
[#33]: https://github.com/mehcode/config-rs/pull/33

## 0.5.1 – 2017-06-16
 - Added config category to Cargo.toml

## 0.5.0 – 2017-06-16
 - `config.get` has been changed to take a type parameter and to deserialize into that type using serde. Old behavior (get a value variant) can be used by passing `config::Value` as the type parameter: `my_config.get::<config::Value>("..")`. Some great help here from [@impowski] in [#25].
 - Propagate parse and type errors through the deep merge (remembering filename, line, etc.)
 - Remove directory traversal on `File`. This is likely temporary. I do _want_ this behavior but I can see how it should be optional. See [#35]
 - Add `File::with_name` to get automatic file format detection instead of manual `FileFormat::*` – [@JordiPolo]
 - Case normalization [#26]
 - Remove many possible panics [#8]
 - `my_config.refresh()` will do a full re-read from the source so live configuration is possible with some work to watch the file

[#8]: https://github.com/mehcode/config-rs/issues/8
[#35]: https://github.com/mehcode/config-rs/pull/35
[#26]: https://github.com/mehcode/config-rs/pull/26
[#25]: https://github.com/mehcode/config-rs/pull/25

[@impowski]: https://github.com/impowski
[@JordiPolo]: https://github.com/JordiPolo

## 0.4.0 - 2017-02-12
 - Remove global ( `config::get` ) API — It's now required to create a local configuration instance with `config::Config::new()` first.

   If you'd like to have a global configuration instance, use `lazy_static!` as follows:

   ```rust
   use std::sync::RwLock;
   use config::Config;

   lazy_static! {
       static ref CONFIG: RwLock<Config> = Default::default();
   }
   ```

## 0.3.0 - 2017-02-08
 - YAML from [@tmccombs](https://github.com/tmccombs)
 - Nested field retrieval
 - Deep merging of sources (was shallow)
 - `config::File::from_str` to parse and merge a file from a string
 - Support for retrieval of maps and slices — `config::get_table` and `config::get_array`

## 0.2.0 - 2017-01-29
Initial release.
