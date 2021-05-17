winreg
[![Winreg on Appveyor][appveyor-image]][appveyor]
[![Winreg on crates.io][cratesio-image]][cratesio]
[![Winreg on docs.rs][docsrs-image]][docsrs]
======

[appveyor-image]: https://ci.appveyor.com/api/projects/status/f3lwrt67ghrf5omd?svg=true
[appveyor]: https://ci.appveyor.com/project/gentoo90/winreg-rs
[cratesio-image]: https://img.shields.io/crates/v/winreg.svg
[cratesio]: https://crates.io/crates/winreg
[docsrs-image]: https://docs.rs/winreg/badge.svg
[docsrs]: https://docs.rs/winreg

Rust bindings to MS Windows Registry API. Work in progress.

Current features:
* Basic registry operations:
    * open/create/delete keys
    * read and write values
    * seamless conversion between `REG_*` types and rust primitives
        * `String` and `OsString` <= `REG_SZ`, `REG_EXPAND_SZ` or `REG_MULTI_SZ`
        * `String`, `&str` and `OsStr` => `REG_SZ`
        * `u32` <=> `REG_DWORD`
        * `u64` <=> `REG_QWORD`
* Iteration through key names and through values
* Transactions
* Transacted serialization of rust types into/from registry (only primitives and structures for now)

## Usage

### Basic usage

```toml
# Cargo.toml
[dependencies]
winreg = "0.7"
```

```rust
extern crate winreg;
use std::io;
use std::path::Path;
use winreg::enums::*;
use winreg::RegKey;

fn main() -> io::Result<()> {
    println!("Reading some system info...");
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let cur_ver = hklm.open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion")?;
    let pf: String = cur_ver.get_value("ProgramFilesDir")?;
    let dp: String = cur_ver.get_value("DevicePath")?;
    println!("ProgramFiles = {}\nDevicePath = {}", pf, dp);
    let info = cur_ver.query_info()?;
    println!("info = {:?}", info);
    let mt = info.get_last_write_time_system();
    println!(
        "last_write_time as winapi::um::minwinbase::SYSTEMTIME = {}-{:02}-{:02} {:02}:{:02}:{:02}",
        mt.wYear, mt.wMonth, mt.wDay, mt.wHour, mt.wMinute, mt.wSecond
    );

    // enable `chrono` feature on `winreg` to make this work
    // println!(
    //     "last_write_time as chrono::NaiveDateTime = {}",
    //     info.get_last_write_time_chrono()
    // );

    println!("And now lets write something...");
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = Path::new("Software").join("WinregRsExample1");
    let (key, disp) = hkcu.create_subkey(&path)?;

    match disp {
        REG_CREATED_NEW_KEY => println!("A new key has been created"),
        REG_OPENED_EXISTING_KEY => println!("An existing key has been opened"),
    }

    key.set_value("TestSZ", &"written by Rust")?;
    let sz_val: String = key.get_value("TestSZ")?;
    key.delete_value("TestSZ")?;
    println!("TestSZ = {}", sz_val);

    key.set_value("TestDWORD", &1234567890u32)?;
    let dword_val: u32 = key.get_value("TestDWORD")?;
    println!("TestDWORD = {}", dword_val);

    key.set_value("TestQWORD", &1234567891011121314u64)?;
    let qword_val: u64 = key.get_value("TestQWORD")?;
    println!("TestQWORD = {}", qword_val);

    key.create_subkey("sub\\key")?;
    hkcu.delete_subkey_all(&path)?;

    println!("Trying to open nonexistent key...");
    hkcu.open_subkey(&path).unwrap_or_else(|e| match e.kind() {
        io::ErrorKind::NotFound => panic!("Key doesn't exist"),
        io::ErrorKind::PermissionDenied => panic!("Access denied"),
        _ => panic!("{:?}", e),
    });
    Ok(())
}
```

### Iterators

```rust
extern crate winreg;
use std::io;
use winreg::RegKey;
use winreg::enums::*;

fn main() -> io::Result<()> {
    println!("File extensions, registered in system:");
    for i in RegKey::predef(HKEY_CLASSES_ROOT)
        .enum_keys().map(|x| x.unwrap())
        .filter(|x| x.starts_with("."))
    {
        println!("{}", i);
    }

    let system = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey("HARDWARE\\DESCRIPTION\\System")?;
    for (name, value) in system.enum_values().map(|x| x.unwrap()) {
        println!("{} = {:?}", name, value);
    }

    Ok(())
}
```

### Transactions

```toml
# Cargo.toml
[dependencies]
winreg = { version = "0.7", features = ["transactions"] }
```

```rust
extern crate winreg;
use std::io;
use winreg::RegKey;
use winreg::enums::*;
use winreg::transaction::Transaction;

fn main() -> io::Result<()> {
    let t = Transaction::new()?;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _disp) = hkcu.create_subkey_transacted("Software\\RustTransaction", &t)?;
    key.set_value("TestQWORD", &1234567891011121314u64)?;
    key.set_value("TestDWORD", &1234567890u32)?;

    println!("Commit transaction? [y/N]:");
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    input = input.trim_right().to_owned();
    if input == "y" || input == "Y" {
        t.commit()?;
        println!("Transaction committed.");
    }
    else {
        // this is optional, if transaction wasn't committed,
        // it will be rolled back on disposal
        t.rollback()?;

        println!("Transaction wasn't committed, it will be rolled back.");
    }

    Ok(())
}
```

### Serialization

```toml
# Cargo.toml
[dependencies]
winreg = { version = "0.7", features = ["serialization-serde"] }
serde = "1"
serde_derive = "1"
```

```rust
#[macro_use]
extern crate serde_derive;
extern crate winreg;
use std::error::Error;
use winreg::enums::*;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Coords {
    x: u32,
    y: u32,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Size {
    w: u32,
    h: u32,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Rectangle {
    coords: Coords,
    size: Size,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Test {
    t_bool: bool,
    t_u8: u8,
    t_u16: u16,
    t_u32: u32,
    t_u64: u64,
    t_usize: usize,
    t_struct: Rectangle,
    t_string: String,
    t_i8: i8,
    t_i16: i16,
    t_i32: i32,
    t_i64: i64,
    t_isize: isize,
    t_f64: f64,
    t_f32: f32,
}

fn main() -> Result<(), Box<Error>> {
    let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
    let (key, _disp) = hkcu.create_subkey("Software\\RustEncode")?;
    let v1 = Test{
        t_bool: false,
        t_u8: 127,
        t_u16: 32768,
        t_u32: 123456789,
        t_u64: 123456789101112,
        t_usize: 1234567891,
        t_struct: Rectangle{
            coords: Coords{ x: 55, y: 77 },
            size: Size{ w: 500, h: 300 },
        },
        t_string: "test 123!".to_owned(),
        t_i8: -123,
        t_i16: -2049,
        t_i32: 20100,
        t_i64: -12345678910,
        t_isize: -1234567890,
        t_f64: -0.01,
        t_f32: 3.14,
    };

    key.encode(&v1)?;

    let v2: Test = key.decode()?;
    println!("Decoded {:?}", v2);

    println!("Equal to encoded: {:?}", v1 == v2);
    Ok(())
}
```

## Changelog

### 0.7.0

* Breaking change: remove deprecated `Error::description` ([#28](https://github.com/gentoo90/winreg-rs/pull/28))
* Optimize `Iterator::nth()` for the `Enum*` iterators ([#29](https://github.com/gentoo90/winreg-rs/pull/29))

### 0.6.2

* Add `RegKey::delete_subkey_with_flags()` ([#27](https://github.com/gentoo90/winreg-rs/pull/27))

### 0.6.1

* Add `last_write_time` field to `RegKeyMetadata` (returned by `RegKey::query_info()`) ([#25](https://github.com/gentoo90/winreg-rs/pull/25)).
* Add `get_last_write_time_system()` and `get_last_write_time_chrono()` (under `chrono` feature) methods to `RegKeyMetadata`.

### 0.6.0

* Breaking change: `create_subkey`, `create_subkey_with_flags`, `create_subkey_transacted` and
`create_subkey_transacted_with_flags` now return a tuple which contains the subkey and its disposition
which can be `REG_CREATED_NEW_KEY` or `REG_OPENED_EXISTING_KEY` ([#21](https://github.com/gentoo90/winreg-rs/issues/21)).
* Examples fixed to not use `unwrap` according to [Rust API guidelines](https://rust-lang-nursery.github.io/api-guidelines/documentation.html#examples-use--not-try-not-unwrap-c-question-mark).

### 0.5.1

* Reexport `HKEY` ([#15](https://github.com/gentoo90/winreg-rs/issues/15)).
* Add `raw_handle` method ([#18](https://github.com/gentoo90/winreg-rs/pull/18)).

### 0.5.0

* Breaking change: `open_subkey` now opens a key with readonly permissions.
Use `create_subkey` or `open_subkey_with_flags` to open with read-write permissins.
* Breaking change: features `transactions` and `serialization-serde` are now disabled by default.
* Breaking change: serialization now uses `serde` instead of `rustc-serialize`.
* `winapi` updated to `0.3`.
* Documentation fixes ([#14](https://github.com/gentoo90/winreg-rs/pull/14))

### 0.4.0

* Make transactions and serialization otional features
* Update dependensies + minor fixes ([#12](https://github.com/gentoo90/winreg-rs/pull/12))

### 0.3.5

* Implement `FromRegValue` for `OsString` and `ToRegValue` for `OsStr` ([#8](https://github.com/gentoo90/winreg-rs/issues/8))
* Minor fixes

### 0.3.4

* Add `copy_tree` method to `RegKey`
* Now checked with [rust-clippy](https://github.com/Manishearth/rust-clippy)
    * no more `unwrap`s
    * replaced `to_string` with `to_owned`
* Fix: reading strings longer than 2048 characters ([#6](https://github.com/gentoo90/winreg-rs/pull/6))

### 0.3.3

* Fix: now able to read values longer than 2048 bytes ([#3](https://github.com/gentoo90/winreg-rs/pull/3))

### 0.3.2

* Fix: `FromRegValue` trait now requires `Sized` (fixes build with rust 1.4)

### 0.3.1

* Fix: bump `winapi` version to fix build

### 0.3.0

* Add transactions support and make serialization transacted
* Breaking change: use `std::io::{Error,Result}` instead of own `RegError` and `RegResult`
