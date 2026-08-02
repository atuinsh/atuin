# Changelog

## v2.0.1

Fixes an issue in 2.0.0 where reqwest was inadvertently built without TLS support.

## v2.0.0

This release updates several dependencies.

### ⚠️ Breaking changes

Update reqwest to 0.13, and consequently remove the `tls-native-roots` feature.

## v1.5.0 - 2025-12-30

### ⚠️ Breaking changes

This release replaces the discontinued [serde_yml](https://crates.io/crates/serde_yml) with [serde_yaml_bw](https://crates.io/crates/serde_yaml_bw).

## v1.4.0 - 2025-08-25

### 🛠️ Fixes

ZIP compression now correctly preserves Unix permissions, including execute. [PR/311]

### 🎁 Features

ZIP files now use DEFLATE compression. Previous versions produced uncompressed ZIP files. [PR/312]

[PR/311]: https://github.com/axodotdev/axoasset/pull/311
[PR/312]: https://github.com/axodotdev/axoasset/pull/312

### Maintenace

Updated dependencies.

## v1.3.0 - 2025-07-20

This release just updates dependencies.

## v1.2.0 - 2024-12-09

This release just updates dependencies.

## v1.1.0 - 2024-12-04

This release adds support for deserializing YAML in the same way as TOML. [PR/235]

[PR/235]: https://github.com/axodotdev/axoasset/pull/235

## v1.0.1 - 2024-10-30

The primary feature of this release is adding some internal-for-now environment variables that allow the end user to change the level of compression at runtime.
The primary motivator of this is improving the speed of testing dist. [PR/212]

This release also includes some general dependency updates.

[PR/212]: https://github.com/axodotdev/axoasset/pull/212

## v1.0.0 - 2024-07-05

The design of APIs has been massively overhauled and normalized, with the changes too substantial to individually enumerate. Major highlights:

* Asset (the union between LocalAsset and RemoteAsset) has been removed
* RemoteAsset is largely replaced with AxoClient which to allow you to actually initialized/configure the underlying reqwest client
* Errors cleaned up
* Function names cleaned up to be unambiguous and normal
* "missing" APIs added


## v0.10.1 - 2024-06-10

Fixes the `pub use reqwest` that was added in the previous version.

### 🛠️ Fixes

### v0.10.0 - 2024-06-06

### 🛠️ Fixes

- **RemoteAsset: fix mimetype requirement - [mistydemeo], [pr126]**

Fixes an issue where functions like `RemoteAsset::copy` would fail on files without specific mimetypes. We used this to assign file extensions based on mimetype, but it shouldn't have rejected other files.

[pr126]: https://github.com/axodotdev/axoasset/pull/126

- **RemoteAsset: exposes reqwest - [mistydemeo], [pr137]**

[pr137]: https://github.com/axodotdev/axoasset/pull/137

- **LocalAsset: fixes a misleading error message - sorairolake, [pr126]**

[pr133]: https://github.com/axodotdev/axoasset/pull/133

### Maintenace

Updates several dependencies.

### v0.9.5 - 2024-05-22

### Maintenace

Relaxes the `reqwest` dependency range.

### v0.9.4 - 2024-05-22

### Maintenace

Updates several dependencies.

### v0.9.3 - 2024-04-16

### 🛠️ Fixes

Reduces the dependency tree when the `remote` feature isn't in use by properly scoping the `image` dependency.

### v0.9.2 - 2024-04-15

### 🛠️ Fixes

Fixes a branching error in the previous release which prevented the ZIP fix from being usable.

### v0.9.1 - 2024-03-26

### 🛠️ Fixes

- **Zipping directory trees on Windows - [mistydemeo], [pr94]**

Recursive directory trees on Windows would be zipped with mangled filenames; this has been fixed by preprocessing the file names before passing them to the `zip` crate.

[pr94]: https://github.com/axodotdev/axoasset/pull/94

### v0.9.0 - 2024-03-14

### 🎁 Features

- **Parsing JSON containing byte order marks - [mistydemeo], [pr87]**

This fixes an issue parsing JSON from files containing a [byte order mark]. This is rare, but can occur with JSON files created in Windows with certain software, including data written to disk in PowerShell.

The underlying JSON parsing library used by axoasset doesn't currently support parsing JSON files that begin with a byte order mark. In this release, we strip it from files that contain it before passing it to serde in order to work around this limitation.

[byte order mark]: https://en.wikipedia.org/wiki/Byte_order_mark
[pr87]: https://github.com/axodotdev/axoasset/pull/87

### v0.8.0 - 2024-03-06

### 🎁 Features

- **Extract archives - [mistydemeo], [pr84]**

Adds the ability to decompress tarballs and ZIP files from `LocalAsset`. Users can extract an entire archive to a directory via the `untar_gz_all`/`untar_xz_all`/`untar_zstd_all`/`unzip_all` methods, or extract individual files to bytearrays of their contents via the `untar_gz_file`/`untar_xz_file`/`untar_zstd_file`/`unzip_file` metods.

[pr84]: https://github.com/axodotdev/axoasset/pull/84

### v0.7.0 - 2024-02-15

Updates dependencies, including a breaking upgrade to miette. Users of this crate will need to update to at least miette 6.0.0.

### v0.6.2 - 2024-01-23

Fixes zstd compression to actually use zstd, whoops!


### v0.6.1 - 2023-12-19

Minor updates to dependencies to reduce the amount of compression libraries we dynamically link.


### v0.6.0 - 2023-10-31

### 🎁 Features

- **New reexports - [mistydemeo], [pr68]**

  Reexports `toml`, `toml_edit` and `serde_json`. Types from these three crates
  appear in certain axoasset function signatures.

[pr68]: https://github.com/axodotdev/axoasset/pull/68

### v0.5.1 - 2023-09-14

### 🛠️  Fixes

- **Reduce dependency tree size - [mistydemeo], [pr66]**

  Reduces the size of axoasset's dependency tree by not installing unused
  features from the `images` dependency.

### v0.5.0 - 2023-08-08

### 🎁 Features

- **Add a with_root argument to compression methods - [Gankra], [pr61]**

  The compression methods take a path to a directory to tar/zip up. The
  with_root argument specifies a root prefix of directories that the
  archive's contents should be nested under. If None then the dir's contents
  are flattened into the root of the archive.

  e.g. to make a tar.gz that matches the npm package format (which
  wants the tarball to contain a dir named "package"), you can
  compress: `"path/to/contents/", Some("package")`

- **Add more copying APIs to LocalAsset - [Gankra], [pr62]**

  LocalAsset now includes `copy_named`, `copy_dir`, and `copy_dir_named`.
  All `copy` functions were change to return a `Utf8PathBuf` instead of a `PathBuf`.

[pr61]: https://github.com/axodotdev/axoasset/pull/61
[pr62]: https://github.com/axodotdev/axoasset/pull/62

## v0.4.0 - 2023-07-04

### 🎁 Features

- **Don't use OpenSSL - [Gankra], [pr56]**

### 🛠️  Fixes

- **Don't reject spans that cover the last char - [Gankra], [pr55]**

[pr55]: https://github.com/axodotdev/axoasset/pull/55
[pr56]: https://github.com/axodotdev/axoasset/pull/56

## v0.3.0 - 2023-05-23

### 🎁 Features

- **SourceFile::deserialize_toml_edit (behind new toml-edit feature) - [Gankra], [pr52]**

  Just a convenience to read a SourceFile as toml-edit and map the error spans to the right format.

### 🛠️ Fixes

- **Separate compression into cargo features - [shadows-withal], [pr47]**

  The APIs for processing tarballs/zips are now behind "compression-tar" and "compression-zip",
  with a convenience "compression" feature that covers both.

- **LocalAsset API cleanup - [shadows-withal], [pr48]**

  Some breaking cleanups to APIs to make them more ergonomic longterm

  - Many APIs that previously took Strings now take `AsRef<Utf8Path>`
  - write_new_{all} now just takes a path to the file, instead of folder_path + name

- **update github CI - [striezel], [pr50]**

  Updating several old Github CI actions to more modern/maintained versions, thanks a ton!

* **fix typos - [striezel], [pr51]**

  Thanks!!

[pr47]: https://github.com/axodotdev/axoasset/pull/47
[pr48]: https://github.com/axodotdev/axoasset/pull/48
[pr50]: https://github.com/axodotdev/axoasset/pull/50
[pr51]: https://github.com/axodotdev/axoasset/pull/51
[pr52]: https://github.com/axodotdev/axoasset/pull/52

## v0.2.0 - 2023-04-27

### 🎁 Features

- **✨ New `LocalAsset` functionality! - [shadows-withal], [pr38], [pr46]**

  We've added a lot more functions to `LocalAsset`:

  - `write_new_all`, to write a file and its parent directories
  - `create_dir`, which creates, well, a new directory
  - `create_dir_all`, which creates a directory and its parent directories
  - `remove_file`, which deletes a file
  - `remove_dir`, which deletes an empty directory
  - `remove_dir_all`, which deletes a directory and its contents
  - `tar_{gz,xz,zstd}_dir`, which are three separate functions that create a tar archive with the
    specified compression algorithm, either Gzip, Xzip, or Zstd
  - `zip_dir`, which creates a zip archive

- **✨ New feature: `SourceFile::span_for_substr` - [Gankra], [pr35]**

  This function enables the ability to get spans even when using a tool that
  doesn't support them as long as it returns actual substrings pointing into
  the original SourceFile's inner String.

### 🛠️ Fixes

- **Simply SourceFile::new and new_empty - [Gankra], [pr43]**

  SourceFile::new and new_empty no longer return Results and simply use the origin_path
  as the file name, making them appropriate for synthetic/test inputs that don't map
  to actual files.

[pr35]: https://github.com/axodotdev/axoasset/pull/35
[pr43]: https://github.com/axodotdev/axoasset/pull/43
[pr38]: https://github.com/axodotdev/axoasset/pull/38
[pr46]: https://github.com/axodotdev/axoasset/pull/46


## v0.1.1 - 2023-04-06

### 🛠️  Fixes

- **Fix compilation errors for features and add tests - [Gankra]/[ashleygwilliams], [pr33]**

[pr33]: https://github.com/axodotdev/axoasset/pull/33

## v0.1.0 - 2023-04-06

### 🎁 Features

- **✨ New type: `SourceFile` - [Gankra],  [pr25]**

  `SourceFile` is a new asset type which is a readonly String version of
  `Asset` wrapped in an `Arc`. The purpose of this type is to be cheap to
  clone and pass around everywhere so that errors can refer to it (using the
  miette `#[source_code]` and `#[label]` attributes). The `Arc` ensures this
  is cheap at minimal overhead. The String ensures the contents make sense to
  display.

- **✨ New type: `Spanned` - [Gankra],  [pr25]**

  `Spanned<T>` is a new type which tries to behave like `Box<T>` in the sense
  that it's "as if" it's a `T` but with source span info embedded. If you want
  to remember that a value was decoded from an asset at bytes 100 to 200, you
  can wrap it in a `Spanned` without disrupting any of the code that uses it.
  Then if you determine that value caused a problem, you can call
  `Spanned::span(&value)` to extract the span and have miette include the
  asset context in the error message.

- **✨ New features: `serde_json` and `toml-rs` - [Gankra],  [pr25]**

  `json-serde` and `toml-serde` are new features which pull in dedicated
  support for `serde_json` and `toml-rs`. These features add `deserialize_json`
  and `deserialize_toml` methods to `SourceFile` which understand those crates'
  native error types and produce full pretty miette-y errors when deserializing,
  like this:

  ```
    × failed to read JSON
    ╰─▶ trailing comma at line 3 column 1
     ╭─[src/tests/res/bad-package.json:2:1]
   2 │     "name": null,
   3 │ }
     · ─
     ╰────
  ```

  (In this case serde_json itself points at the close brace and not the actual comma, we're just faithfully forwarding that.)

  `Spanned` has special integration with `toml-rs`, because it's actually a
  fork of that crate's [own magic `Spanned` type]. If you deserialize a struct
  that contains a `Spanned<T>` it will automagically fill in the span info
  for you. Ours further improves on this by putting in more effort to be totally
  transparent like `Box`.

- **✨ New function: `write_new` for `LocalAsset` - [ashleygwilliams], [pr28]**

  axoasset was first conceived to handle assets declared by end users for use
  in `oranda`, but quickly grew to encompass all fs/network calls. one of the
  things we often need to do is create a new file. This is only available on
  `LocalAsset` as, at least for the moment, that is the only place axoasset
  has permissions to create new assets.

- **make `RemoteAsset` an optional feature - [Gankra], [pr26]**

  A feature of `axoasset` is that it is agnostic to the origin of the asset:
  it can be local or remote. However, often, authors can be certain that they
  will only be using local assets. In this case, it reduces dependencies to
  not include the remote functionality. Previously this wasn't possible!

- **`miette-ify` errors - [Gankra], [pr24]**

  Previously we were using `thiserror` for error handling, but to be consistent
  across our toolchain, we've updated our errors to use `miette`. This has the
  added benefit of formalizing structures we were informally building into our
  error types (help/diagnostic text, forwarding the bare error as details, etc).


- **consistent `Asset` interface - [ashleygwilliams], [pr30]**

  With 3 asset types, `LocalAsset`, `RemoteAsset`, and `SourceFile`, it felt
  important to align their structures so they could be used nearly identically.
  Every type now has a:

     - `origin_path`: the original source of the file
     - `filename`: derived from the `origin_path` and, in the case of `RemoteAsset`s
        also the headers from the network response.
     - `contents`: the contents of the asset as bytes or a String depending on
        asset type

[pr24]: https://github.com/axodotdev/axoasset/pull/24
[pr25]: https://github.com/axodotdev/axoasset/pull/25
[pr26]: https://github.com/axodotdev/axoasset/pull/26
[pr28]: https://github.com/axodotdev/axoasset/pull/28
[pr30]: https://github.com/axodotdev/axoasset/pull/30

[own magic `Spanned` type]: https://docs.rs/toml/latest/toml/struct.Spanned.html

## v0.0.1 - 2023-02-14

Initial release.

[ashleygwilliams]: https://github.com/ashleygwilliams
[gankra]: https://github.com/gankra
[mistydemeo]: https://github.com/mistydemeo
[shadows-withal]: https://github.com/shadows-withal
[striezel]: https://github.com/striezel
