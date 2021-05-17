[![crates.io](https://img.shields.io/crates/v/directories.svg)](https://crates.io/crates/directories)
[![API documentation](https://docs.rs/directories/badge.svg)](https://docs.rs/directories/)
![actively developed](https://img.shields.io/badge/maintenance-actively--developed-brightgreen.svg)
[![TravisCI status](https://img.shields.io/travis/dirs-dev/directories-rs/master.svg?label=Linux/macOS%20build)](https://travis-ci.org/dirs-dev/directories-rs)
[![AppVeyor status](https://img.shields.io/appveyor/ci/soc/directories-rs/master.svg?label=Windows%20build)](https://ci.appveyor.com/project/soc/directories-rs/branch/master)
![License: MIT/Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-orange.svg)

# `directories`

## Introduction

- a tiny mid-level library with a minimal API
- that provides the platform-specific, user-accessible locations
- for retrieving and storing configuration, cache and other data
- on Linux, Redox, Windows (≥ Vista), macOS and other platforms.

The library provides the location of these directories by leveraging the mechanisms defined by
- the [XDG base directory](https://standards.freedesktop.org/basedir-spec/basedir-spec-latest.html) and
  the [XDG user directory](https://www.freedesktop.org/wiki/Software/xdg-user-dirs/) specifications on Linux
- the [Known Folder](https://msdn.microsoft.com/en-us/library/windows/desktop/dd378457.aspx) API on Windows
- the [Standard Directories](https://developer.apple.com/library/content/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html#//apple_ref/doc/uid/TP40010672-CH2-SW6)
  guidelines on macOS

## Platforms

This library is written in Rust, and supports Linux, Redox, macOS and Windows.
Other platforms are also supported; they use the Linux conventions.

_dirs_, the low-level sister library, is available at [dirs-rs](https://github.com/soc/dirs-rs).

A version of this library running on the JVM is provided by [directories-jvm](https://github.com/soc/directories-jvm).

## Usage

#### Dependency

Add the library as a dependency to your project by inserting

```toml
directories = "3.0"
```

into the `[dependencies]` section of your Cargo.toml file.

If you are upgrading from version 2, please read the [section on breaking changes](#3) first.

#### Example

Library run by user Alice:

```rust
extern crate directories;
use directories::{BaseDirs, UserDirs, ProjectDirs};

if let Some(proj_dirs) = ProjectDirs::from("com", "Foo Corp",  "Bar App") {
    proj_dirs.config_dir();
    // Lin: /home/alice/.config/barapp
    // Win: C:\Users\Alice\AppData\Roaming\Foo Corp\Bar App\config
    // Mac: /Users/Alice/Library/Application Support/com.Foo-Corp.Bar-App
}

if let Some(base_dirs) = BaseDirs::new() {
    base_dirs.executable_dir();
    // Lin: Some(/home/alice/.local/bin)
    // Win: None
    // Mac: None
}

if let Some(user_dirs) = UserDirs::new() {
    user_dirs.audio_dir();
    // Lin: /home/alice/Music
    // Win: C:\Users\Alice\Music
    // Mac: /Users/Alice/Music
}
```

## Design Goals

- The _directories_ library is designed to provide an accurate snapshot of the system's state at
  the point of invocation of `BaseDirs::new`, `UserDirs::new` or `ProjectDirs::from`.<br/>
  Subsequent changes to the state of the system are not reflected in values created prior to such a change.
- This library does not create directories or check for their existence. The library only provides
  information on what the path to a certain directory _should_ be.<br/>
  How this information is used is a decision that developers need to make based on the requirements
  of each individual application.
- This library is intentionally focused on providing information on user-writable directories only,
  as there is no discernible benefit in returning a path that points to a user-level, writable
  directory on one operating system, but a system-level, read-only directory on another.<br/>
  The confusion and unexpected failure modes of such an approach would be immense.
  - `executable_dir` is specified to provide the path to a user-writable directory for binaries.<br/>
    As such a directory only commonly exists on Linux, it returns `None` on macOS and Windows.
  - `font_dir` is specified to provide the path to a user-writable directory for fonts.<br/>
    As such a directory only exists on Linux and macOS, it returns `None` on Windows.
  - `runtime_dir` is specified to provide the path to a directory for non-essential runtime data.
    It is required that this directory is created when the user logs in, is only accessible by the
    user itself, is deleted when the user logs out, and supports all filesystem features of the
    operating system.<br/>
    As such a directory only commonly exists on Linux, it returns `None` on macOS and Windows.

## Features

### `BaseDirs`

The intended use case for `BaseDirs` is to query the paths of user-invisible standard directories
that have been defined according to the conventions of the operating system the library is running on.

If you want to compute the location of cache, config or data directories for your own application or project, use `ProjectDirs` instead.

| Function name    | Value on Linux                                                                                  | Value on Windows            | Value on macOS                      |
| ---------------- | ----------------------------------------------------------------------------------------------- | --------------------------- | ----------------------------------- |
| `home_dir`       | `$HOME`                                                                                         | `{FOLDERID_Profile}`        | `$HOME`                             |
| `cache_dir`      | `$XDG_CACHE_HOME`              or `$HOME`/.cache                                                | `{FOLDERID_LocalAppData}`   | `$HOME`/Library/Caches              |
| `config_dir`     | `$XDG_CONFIG_HOME`             or `$HOME`/.config                                               | `{FOLDERID_RoamingAppData}` | `$HOME`/Library/Application Support |
| `data_dir`       | `$XDG_DATA_HOME`               or `$HOME`/.local/share                                          | `{FOLDERID_RoamingAppData}` | `$HOME`/Library/Application Support |
| `data_local_dir` | `$XDG_DATA_HOME`               or `$HOME`/.local/share                                          | `{FOLDERID_LocalAppData}`   | `$HOME`/Library/Application Support |
| `executable_dir` | `Some($XDG_BIN_HOME`/../bin`)` or `Some($XDG_DATA_HOME`/../bin`)` or `Some($HOME`/.local/bin`)` | `None`                      | `None`                              |
| `preference_dir` | `$XDG_CONFIG_HOME`             or `$HOME`/.config                                               | `{FOLDERID_RoamingAppData}` | `$HOME`/Library/Preferences         |
| `runtime_dir`    | `Some($XDG_RUNTIME_DIR)`       or `None`                                                        | `None`                      | `None`                              |

### `UserDirs`

The intended use case for `UserDirs` is to query the paths of user-facing standard directories
that have been defined according to the conventions of the operating system the library is running on.

| Function name    | Value on Linux                                                         | Value on Windows                 | Value on macOS                 |
| ---------------- | ---------------------------------------------------------------------- | -------------------------------- | ------------------------------ |
| `home_dir`       | `$HOME`                                                                | `{FOLDERID_Profile}`             | `$HOME`                        |
| `audio_dir`      | `Some(XDG_MUSIC_DIR)`           or `None`                              | `Some({FOLDERID_Music})`         | `Some($HOME`/Music/`)`         |
| `desktop_dir`    | `Some(XDG_DESKTOP_DIR)`         or `None`                              | `Some({FOLDERID_Desktop})`       | `Some($HOME`/Desktop/`)`       |
| `document_dir`   | `Some(XDG_DOCUMENTS_DIR)`       or `None`                              | `Some({FOLDERID_Documents})`     | `Some($HOME`/Documents/`)`     |
| `download_dir`   | `Some(XDG_DOWNLOAD_DIR)`        or `None`                              | `Some({FOLDERID_Downloads})`     | `Some($HOME`/Downloads/`)`     |
| `font_dir`       | `Some($XDG_DATA_HOME`/fonts/`)` or `Some($HOME`/.local/share/fonts/`)` | `None`                           | `Some($HOME`/Library/Fonts/`)` |
| `picture_dir`    | `Some(XDG_PICTURES_DIR)`        or `None`                              | `Some({FOLDERID_Pictures})`      | `Some($HOME`/Pictures/`)`      |
| `public_dir`     | `Some(XDG_PUBLICSHARE_DIR)`     or `None`                              | `Some({FOLDERID_Public})`        | `Some($HOME`/Public/`)`        |
| `template_dir`   | `Some(XDG_TEMPLATES_DIR)`       or `None`                              | `Some({FOLDERID_Templates})`     | `None`                         | 
| `video_dir`      | `Some(XDG_VIDEOS_DIR)`          or `None`                              | `Some({FOLDERID_Videos})`        | `Some($HOME`/Movies/`)`        |

### `ProjectDirs`

The intended use case for `ProjectDirs` is to compute the location of cache, config or data directories for your own application or project,
which are derived from the standard directories.

| Function name    | Value on Linux                                                                     | Value on Windows                                    | Value on macOS                                       |
| ---------------- | ---------------------------------------------------------------------------------- | --------------------------------------------------- | ---------------------------------------------------- |
| `cache_dir`      | `$XDG_CACHE_HOME`/`<project_path>`        or `$HOME`/.cache/`<project_path>`       | `{FOLDERID_LocalAppData}`/`<project_path>`/cache    | `$HOME`/Library/Caches/`<project_path>`              |
| `config_dir`     | `$XDG_CONFIG_HOME`/`<project_path>`       or `$HOME`/.config/`<project_path>`      | `{FOLDERID_RoamingAppData}`/`<project_path>`/config | `$HOME`/Library/Application Support/`<project_path>` |
| `data_dir`       | `$XDG_DATA_HOME`/`<project_path>`         or `$HOME`/.local/share/`<project_path>` | `{FOLDERID_RoamingAppData}`/`<project_path>`/data   | `$HOME`/Library/Application Support/`<project_path>` |
| `data_local_dir` | `$XDG_DATA_HOME`/`<project_path>`         or `$HOME`/.local/share/`<project_path>` | `{FOLDERID_LocalAppData}`/`<project_path>`/data     | `$HOME`/Library/Application Support/`<project_path>` |
| `preference_dir` | `$XDG_CONFIG_HOME`/`<project_path>`       or `$HOME`/.config/`<project_path>`      | `{FOLDERID_RoamingAppData}`/`<project_path>`/config | `$HOME`/Library/Preferences/`<project_path>`         |
| `runtime_dir`    | `Some($XDG_RUNTIME_DIR`/`_project_path_)`                                          | `None`                                              | `None`                                               |

The specific value of `<project_path>` is computed by the

    ProjectDirs::from(qualifier: &str,
                      organization: &str,
                      application: &str)

function and varies across operating systems. As an example, calling

    ProjectDirs::from("org"         /*qualifier*/,
                      "Baz Corp"    /*organization*/,
                      "Foo Bar-App" /*application*/)

results in the following values:

| Value on Linux | Value on Windows         | Value on macOS               |
| -------------- | ------------------------ | ---------------------------- |
| `"foobar-app"` | `"Baz Corp/Foo Bar-App"` | `"org.Baz-Corp.Foo-Bar-App"` |

The `ProjectDirs::from_path` function allows the creation of `ProjectDirs` structs directly from a `PathBuf` value.
This argument is used verbatim and is not adapted to operating system standards.

The use of `ProjectDirs::from_path` is strongly discouraged, as its results will not follow operating system standards on at least two of three platforms.

## Comparison

There are other crates in the Rust ecosystem that try similar or related things.
Here is an overview of them, combined with ratings on properties that guided the design of this crate.

Please take this table with a grain of salt: a different crate might very well be more suitable for your specific use case.
(Of course _my_ crate achieves _my_ design goals better than other crates, which might have had different design goals.)

| Library                                                   | Status         | Lin | Mac | Win |Base|User|Proj|Conv|
| --------------------------------------------------------- | -------------- |:---:|:---:|:---:|:--:|:--:|:--:|:--:|
| [app_dirs](https://crates.io/crates/app_dirs)             | Unmaintained   |  ✔  |  ✔  |  ✔  | 🞈  | ✖  | ✔  | ✖  |
| [app_dirs2](https://crates.io/crates/app_dirs2)           | Maintained     |  ✔  |  ✔  |  ✔  | 🞈  | ✖  | ✔  | ✖  |
| [dirs](https://crates.io/crates/dirs)                     | Developed      |  ✔  |  ✔  |  ✔  | ✔  | ✔  | ✖  | ✔  |
| **directories**                                           | **Developed**  |  ✔  |  ✔  |  ✔  | ✔  | ✔  | ✔  | ✔  |
| [s_app_dir](https://crates.io/crates/s_app_dir)           | Unmaintained?  |  ✔  |  ✖  |  🞈  | ✖  | ✖  | 🞈  | ✖  |
| [standard_paths](https://crates.io/crates/standard_paths) | Maintained     |  ✔  |  ✖  |  ✔  | ✔  | ✔  | ✔  | ✖  |
| [xdg](https://crates.io/crates/xdg)                       | Maintained     |  ✔  |  ✖  |  ✖  | ✔  | ✖  | ✔  | 🞈  |
| [xdg-basedir](https://crates.io/crates/xdg-basedir)       | Unmaintained?  |  ✔  |  ✖  |  ✖  | ✔   | ✖  | ✖  | 🞈  |
| [xdg-rs](https://crates.io/crates/xdg-rs)                 | Obsolete       |  ✔  |  ✖  |  ✖  | ✔   | ✖  | ✖  | 🞈  |

- Lin: Linux support
- Mac: macOS support
- Win: Windows support
- Base: Supports [generic base directories](#basedirs)
- User: Supports [user directories](#userdirs)
- Proj: Supports [project-specific base directories](#projectdirs)
- Conv: Follows naming conventions of the operating system it runs on

## Build

It's possible to cross-compile this library if the necessary toolchains are installed with rustup.
This is helpful to ensure a change has not broken compilation on a different platform.

The following commands will build this library on Linux, macOS and Windows:

```
cargo build --target=x86_64-unknown-linux-gnu
cargo build --target=x86_64-pc-windows-gnu
cargo build --target=x86_64-apple-darwin
cargo build --target=x86_64-unknown-redox
```

## Changelog

### 3

- **BREAKING CHANGE** The behavior of the `BaseDirs::config_dir` and `ProjectDirs::config_dir`
    on macOS has been adjusted (thanks to [everyone involved](https://github.com/dirs-dev/directories-rs/issues/62)):
  - The existing `config_dir` functions have been changed to return the `Application Support`
    directory on macOS, as suggested by Apple documentation.
  - The behavior of the `config_dir` functions on non-macOS platforms has not been changed.
  - If you have used the `config_dir` functions to store files, it may be necessary to write code
    that migrates the files to the new location on macOS.<br/>
    (Alternative: change uses of the `config_dir` functions to uses of the `preference_dir` functions
    to retain the old behavior.)
- The newly added `BaseDirs::preference_dir` and `ProjectDirs::preference_dir` functions returns
  the `Preferences` directory on macOS now, which – according to Apple documentation – shall only
  be used to store .plist files using Apple-proprietary APIs.
  – `preference_dir` and `config_dir` behave identical on non-macOS platforms.

### 2

The behavior of deactivated, missing or invalid [_XDG User Dirs_](https://www.freedesktop.org/wiki/Software/xdg-user-dirs/)
entries on Linux has been improved (contributed by @tmiasko, thank you!):

- Version 1 returned the user's home directory (`Some($HOME)`) for such faulty entries, except for a faulty `XDG_DESKTOP_DIR` entry which returned (`Some($HOME/Desktop)`).
- Version 2 returns `None` for such entries.

## License

Licensed under either of

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
