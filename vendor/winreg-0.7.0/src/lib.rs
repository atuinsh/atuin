// Copyright 2015, Igor Shaula
// Licensed under the MIT License <LICENSE or
// http://opensource.org/licenses/MIT>. This file
// may not be copied, modified, or distributed
// except according to those terms.

//! Crate for accessing MS Windows registry
//!
//!## Usage
//!
//!### Basic usage
//!
//!```toml,ignore
//!# Cargo.toml
//![dependencies]
//!winreg = "0.7"
//!```
//!
//!```no_run
//!extern crate winreg;
//!use std::io;
//!use std::path::Path;
//!use winreg::enums::*;
//!use winreg::RegKey;
//!
//!fn main() -> io::Result<()> {
//!    println!("Reading some system info...");
//!    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
//!    let cur_ver = hklm.open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion")?;
//!    let pf: String = cur_ver.get_value("ProgramFilesDir")?;
//!    let dp: String = cur_ver.get_value("DevicePath")?;
//!    println!("ProgramFiles = {}\nDevicePath = {}", pf, dp);
//!    let info = cur_ver.query_info()?;
//!    println!("info = {:?}", info);
//!    let mt = info.get_last_write_time_system();
//!    println!(
//!        "last_write_time as winapi::um::minwinbase::SYSTEMTIME = {}-{:02}-{:02} {:02}:{:02}:{:02}",
//!        mt.wYear, mt.wMonth, mt.wDay, mt.wHour, mt.wMinute, mt.wSecond
//!    );
//!
//!    // enable `chrono` feature on `winreg` to make this work
//!    // println!(
//!    //     "last_write_time as chrono::NaiveDateTime = {}",
//!    //     info.get_last_write_time_chrono()
//!    // );
//!
//!    println!("And now lets write something...");
//!    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
//!    let path = Path::new("Software").join("WinregRsExample1");
//!    let (key, disp) = hkcu.create_subkey(&path)?;
//!
//!    match disp {
//!        REG_CREATED_NEW_KEY => println!("A new key has been created"),
//!        REG_OPENED_EXISTING_KEY => println!("An existing key has been opened"),
//!    }
//!
//!    key.set_value("TestSZ", &"written by Rust")?;
//!    let sz_val: String = key.get_value("TestSZ")?;
//!    key.delete_value("TestSZ")?;
//!    println!("TestSZ = {}", sz_val);
//!
//!    key.set_value("TestDWORD", &1234567890u32)?;
//!    let dword_val: u32 = key.get_value("TestDWORD")?;
//!    println!("TestDWORD = {}", dword_val);
//!
//!    key.set_value("TestQWORD", &1234567891011121314u64)?;
//!    let qword_val: u64 = key.get_value("TestQWORD")?;
//!    println!("TestQWORD = {}", qword_val);
//!
//!    key.create_subkey("sub\\key")?;
//!    hkcu.delete_subkey_all(&path)?;
//!
//!    println!("Trying to open nonexistent key...");
//!    hkcu.open_subkey(&path).unwrap_or_else(|e| match e.kind() {
//!        io::ErrorKind::NotFound => panic!("Key doesn't exist"),
//!        io::ErrorKind::PermissionDenied => panic!("Access denied"),
//!        _ => panic!("{:?}", e),
//!    });
//!    Ok(())
//!}
//!```
//!
//!### Iterators
//!
//!```no_run
//!extern crate winreg;
//!use std::io;
//!use winreg::RegKey;
//!use winreg::enums::*;
//!
//!fn main() -> io::Result<()> {
//!    println!("File extensions, registered in system:");
//!    for i in RegKey::predef(HKEY_CLASSES_ROOT)
//!        .enum_keys().map(|x| x.unwrap())
//!        .filter(|x| x.starts_with("."))
//!    {
//!        println!("{}", i);
//!    }
//!
//!    let system = RegKey::predef(HKEY_LOCAL_MACHINE)
//!        .open_subkey("HARDWARE\\DESCRIPTION\\System")?;
//!    for (name, value) in system.enum_values().map(|x| x.unwrap()) {
//!        println!("{} = {:?}", name, value);
//!    }
//!
//!    Ok(())
//!}
//!```
//!
#[cfg(feature = "chrono")]
extern crate chrono;
#[cfg(feature = "serialization-serde")]
extern crate serde;
extern crate winapi;
use enums::*;
use std::default::Default;
use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::mem::transmute;
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use std::slice;
#[cfg(feature = "transactions")]
use transaction::Transaction;
use types::{FromRegValue, ToRegValue};
pub use winapi::shared::minwindef::HKEY;
use winapi::shared::minwindef::{BYTE, DWORD, FILETIME, LPBYTE};
use winapi::shared::winerror;
use winapi::um::minwinbase::SYSTEMTIME;
use winapi::um::timezoneapi::FileTimeToSystemTime;
use winapi::um::winnt::{self, WCHAR};
use winapi::um::winreg as winapi_reg;

macro_rules! werr {
    ($e:expr) => {
        Err(io::Error::from_raw_os_error($e as i32))
    };
}

#[cfg(feature = "serialization-serde")]
mod decoder;
#[cfg(feature = "serialization-serde")]
mod encoder;
pub mod enums;
#[cfg(feature = "transactions")]
pub mod transaction;
pub mod types;

/// Metadata returned by `RegKey::query_info`
#[derive(Debug, Default)]
pub struct RegKeyMetadata {
    // pub Class: winapi::LPWSTR,
    // pub ClassLen: DWORD,
    pub sub_keys: DWORD,
    pub max_sub_key_len: DWORD,
    pub max_class_len: DWORD,
    pub values: DWORD,
    pub max_value_name_len: DWORD,
    pub max_value_len: DWORD,
    // pub SecurityDescriptor: DWORD,
    pub last_write_time: FILETIME,
}

impl RegKeyMetadata {
    /// Returns `last_write_time` field as `winapi::um::minwinbase::SYSTEMTIME`
    pub fn get_last_write_time_system(&self) -> SYSTEMTIME {
        let mut st: SYSTEMTIME = unsafe { ::std::mem::zeroed() };
        unsafe {
            FileTimeToSystemTime(&self.last_write_time, &mut st);
        }
        st
    }

    /// Returns `last_write_time` field as `chrono::NaiveDateTime`.
    /// Part of `chrono` feature.
    #[cfg(feature = "chrono")]
    pub fn get_last_write_time_chrono(&self) -> chrono::NaiveDateTime {
        let st = self.get_last_write_time_system();

        chrono::NaiveDate::from_ymd(st.wYear.into(), st.wMonth.into(), st.wDay.into()).and_hms(
            st.wHour.into(),
            st.wMinute.into(),
            st.wSecond.into(),
        )
    }
}

/// Raw registry value
#[derive(PartialEq)]
pub struct RegValue {
    pub bytes: Vec<u8>,
    pub vtype: RegType,
}

macro_rules! format_reg_value {
    ($e:expr => $t:ident) => {
        match $t::from_reg_value($e) {
            Ok(val) => format!("{:?}", val),
            Err(_) => return Err(fmt::Error),
        }
    };
}

impl fmt::Debug for RegValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let f_val = match self.vtype {
            REG_SZ | REG_EXPAND_SZ | REG_MULTI_SZ => format_reg_value!(self => String),
            REG_DWORD => format_reg_value!(self => u32),
            REG_QWORD => format_reg_value!(self => u64),
            _ => format!("{:?}", self.bytes), //TODO: implement more types
        };
        write!(f, "RegValue({:?}: {})", self.vtype, f_val)
    }
}

/// Handle of opened registry key
#[derive(Debug)]
pub struct RegKey {
    hkey: HKEY,
}

unsafe impl Send for RegKey {}

impl RegKey {
    /// Open one of predefined keys:
    ///
    /// * `HKEY_CLASSES_ROOT`
    /// * `HKEY_CURRENT_USER`
    /// * `HKEY_LOCAL_MACHINE`
    /// * `HKEY_USERS`
    /// * `HKEY_PERFORMANCE_DATA`
    /// * `HKEY_PERFORMANCE_TEXT`
    /// * `HKEY_PERFORMANCE_NLSTEXT`
    /// * `HKEY_CURRENT_CONFIG`
    /// * `HKEY_DYN_DATA`
    /// * `HKEY_CURRENT_USER_LOCAL_SETTINGS`
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use winreg::RegKey;
    /// # use winreg::enums::*;
    /// let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    /// ```
    pub fn predef(hkey: HKEY) -> RegKey {
        RegKey { hkey }
    }

    /// Return inner winapi HKEY of a key:
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// # use winreg::RegKey;
    /// # use winreg::enums::*;
    /// # fn main() -> Result<(), Box<Error>> {
    /// let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    /// let soft = hklm.open_subkey("SOFTWARE")?;
    /// let handle = soft.raw_handle();
    /// # Ok(())
    /// # }
    /// ```
    pub fn raw_handle(&self) -> HKEY {
        self.hkey
    }

    /// Open subkey with `KEY_READ` permissions.
    /// Will open another handle to itself if `path` is an empty string.
    /// To open with different permissions use `open_subkey_with_flags`.
    /// You can also use `create_subkey` to open with `KEY_ALL_ACCESS` permissions.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// # use winreg::RegKey;
    /// # use winreg::enums::*;
    /// # fn main() -> Result<(), Box<Error>> {
    /// let soft = RegKey::predef(HKEY_CURRENT_USER)
    ///     .open_subkey("Software")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn open_subkey<P: AsRef<OsStr>>(&self, path: P) -> io::Result<RegKey> {
        self.open_subkey_with_flags(path, enums::KEY_READ)
    }

    /// Open subkey with desired permissions.
    /// Will open another handle to itself if `path` is an empty string.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// # use winreg::RegKey;
    /// # use winreg::enums::*;
    /// # fn main() -> Result<(), Box<Error>> {
    /// let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    /// hklm.open_subkey_with_flags("SOFTWARE\\Microsoft", KEY_READ)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn open_subkey_with_flags<P: AsRef<OsStr>>(
        &self,
        path: P,
        perms: winapi_reg::REGSAM,
    ) -> io::Result<RegKey> {
        let c_path = to_utf16(path);
        let mut new_hkey: HKEY = ptr::null_mut();
        match unsafe {
            winapi_reg::RegOpenKeyExW(self.hkey, c_path.as_ptr(), 0, perms, &mut new_hkey) as DWORD
        } {
            0 => Ok(RegKey { hkey: new_hkey }),
            err => werr!(err),
        }
    }

    /// Part of `transactions` feature.
    #[cfg(feature = "transactions")]
    pub fn open_subkey_transacted<P: AsRef<OsStr>>(
        &self,
        path: P,
        t: &Transaction,
    ) -> io::Result<RegKey> {
        self.open_subkey_transacted_with_flags(path, t, winnt::KEY_READ)
    }

    /// Part of `transactions` feature.
    #[cfg(feature = "transactions")]
    pub fn open_subkey_transacted_with_flags<P: AsRef<OsStr>>(
        &self,
        path: P,
        t: &Transaction,
        perms: winapi_reg::REGSAM,
    ) -> io::Result<RegKey> {
        let c_path = to_utf16(path);
        let mut new_hkey: HKEY = ptr::null_mut();
        match unsafe {
            winapi_reg::RegOpenKeyTransactedW(
                self.hkey,
                c_path.as_ptr(),
                0,
                perms,
                &mut new_hkey,
                t.handle,
                ptr::null_mut(),
            ) as DWORD
        } {
            0 => Ok(RegKey { hkey: new_hkey }),
            err => werr!(err),
        }
    }

    /// Create subkey (and all missing parent keys)
    /// and open it with `KEY_ALL_ACCESS` permissions.
    /// Will just open key if it already exists.
    /// If succeeds returns a tuple with the created subkey and its disposition,
    /// which can be `REG_CREATED_NEW_KEY` or `REG_OPENED_EXISTING_KEY`.
    /// Will open another handle to itself if `path` is an empty string.
    /// To create with different permissions use `create_subkey_with_flags`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// # use winreg::RegKey;
    /// use winreg::enums::*;
    /// # fn main() -> Result<(), Box<Error>> {
    /// let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    /// let (settings, disp) = hkcu.create_subkey("Software\\MyProduct\\Settings")?;
    ///
    /// match disp {
    ///     REG_CREATED_NEW_KEY => println!("A new key has been created"),
    ///     REG_OPENED_EXISTING_KEY => println!("An existing key has been opened")
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn create_subkey<P: AsRef<OsStr>>(&self, path: P) -> io::Result<(RegKey, RegDisposition)> {
        self.create_subkey_with_flags(path, enums::KEY_ALL_ACCESS)
    }

    pub fn create_subkey_with_flags<P: AsRef<OsStr>>(
        &self,
        path: P,
        perms: winapi_reg::REGSAM,
    ) -> io::Result<(RegKey, RegDisposition)> {
        let c_path = to_utf16(path);
        let mut new_hkey: HKEY = ptr::null_mut();
        let mut disp_buf: DWORD = 0;
        match unsafe {
            winapi_reg::RegCreateKeyExW(
                self.hkey,
                c_path.as_ptr(),
                0,
                ptr::null_mut(),
                winnt::REG_OPTION_NON_VOLATILE,
                perms,
                ptr::null_mut(),
                &mut new_hkey,
                &mut disp_buf,
            )
        } {
            0 => {
                let disp: RegDisposition = unsafe { transmute(disp_buf as u8) };
                Ok((RegKey { hkey: new_hkey }, disp))
            }
            err => werr!(err),
        }
    }

    /// Part of `transactions` feature.
    #[cfg(feature = "transactions")]
    pub fn create_subkey_transacted<P: AsRef<OsStr>>(
        &self,
        path: P,
        t: &Transaction,
    ) -> io::Result<(RegKey, RegDisposition)> {
        self.create_subkey_transacted_with_flags(path, t, winnt::KEY_ALL_ACCESS)
    }

    /// Part of `transactions` feature.
    #[cfg(feature = "transactions")]
    pub fn create_subkey_transacted_with_flags<P: AsRef<OsStr>>(
        &self,
        path: P,
        t: &Transaction,
        perms: winapi_reg::REGSAM,
    ) -> io::Result<(RegKey, RegDisposition)> {
        let c_path = to_utf16(path);
        let mut new_hkey: HKEY = ptr::null_mut();
        let mut disp_buf: DWORD = 0;
        match unsafe {
            winapi_reg::RegCreateKeyTransactedW(
                self.hkey,
                c_path.as_ptr(),
                0,
                ptr::null_mut(),
                winnt::REG_OPTION_NON_VOLATILE,
                perms,
                ptr::null_mut(),
                &mut new_hkey,
                &mut disp_buf,
                t.handle,
                ptr::null_mut(),
            ) as DWORD
        } {
            0 => {
                let disp: RegDisposition = unsafe { transmute(disp_buf as u8) };
                Ok((RegKey { hkey: new_hkey }, disp))
            }
            err => werr!(err),
        }
    }

    /// Copy all the values and subkeys from `path` to `dest` key.
    /// WIll copy the content of `self` if `path` is an empty string.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// # use winreg::RegKey;
    /// # use winreg::enums::*;
    /// # fn main() -> Result<(), Box<Error>> {
    /// let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    /// let src = hkcu.open_subkey_with_flags("Software\\MyProduct", KEY_READ)?;
    /// let (dst, dst_disp) = hkcu.create_subkey("Software\\MyProduct\\Section2")?;
    /// src.copy_tree("Section1", &dst)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn copy_tree<P: AsRef<OsStr>>(&self, path: P, dest: &RegKey) -> io::Result<()> {
        let c_path = to_utf16(path);
        match unsafe { winapi_reg::RegCopyTreeW(self.hkey, c_path.as_ptr(), dest.hkey) } {
            0 => Ok(()),
            err => werr!(err),
        }
    }

    pub fn query_info(&self) -> io::Result<RegKeyMetadata> {
        let mut info: RegKeyMetadata = Default::default();
        match unsafe {
            winapi_reg::RegQueryInfoKeyW(
                self.hkey,
                ptr::null_mut(), // Class: winapi::LPWSTR,
                ptr::null_mut(), // ClassLen: DWORD,
                ptr::null_mut(), // Reserved
                &mut info.sub_keys,
                &mut info.max_sub_key_len,
                &mut info.max_class_len,
                &mut info.values,
                &mut info.max_value_name_len,
                &mut info.max_value_len,
                ptr::null_mut(), // lpcbSecurityDescriptor: winapi::LPDWORD,
                &mut info.last_write_time,
            ) as DWORD
        } {
            0 => Ok(info),
            err => werr!(err),
        }
    }

    /// Return an iterator over subkeys names.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use winreg::RegKey;
    /// # use winreg::enums::*;
    /// println!("File extensions, registered in this system:");
    /// for i in RegKey::predef(HKEY_CLASSES_ROOT)
    ///     .enum_keys().map(|x| x.unwrap())
    ///     .filter(|x| x.starts_with("."))
    /// {
    ///     println!("{}", i);
    /// }
    /// ```
    pub fn enum_keys(&self) -> EnumKeys {
        EnumKeys {
            key: self,
            index: 0,
        }
    }

    /// Return an iterator over values.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// # use winreg::RegKey;
    /// # use winreg::enums::*;
    /// # fn main() -> Result<(), Box<Error>> {
    /// let system = RegKey::predef(HKEY_LOCAL_MACHINE)
    ///     .open_subkey_with_flags("HARDWARE\\DESCRIPTION\\System", KEY_READ)?;
    /// for (name, value) in system.enum_values().map(|x| x.unwrap()) {
    ///     println!("{} = {:?}", name, value);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn enum_values(&self) -> EnumValues {
        EnumValues {
            key: self,
            index: 0,
        }
    }

    /// Delete key. Key names are not case sensitive.
    /// Cannot delete if it has subkeys.
    /// Use `delete_subkey_all` for that.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// # use winreg::RegKey;
    /// # use winreg::enums::*;
    /// # fn main() -> Result<(), Box<Error>> {
    /// RegKey::predef(HKEY_CURRENT_USER)
    ///     .delete_subkey(r"Software\MyProduct\History")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn delete_subkey<P: AsRef<OsStr>>(&self, path: P) -> io::Result<()> {
        self.delete_subkey_with_flags(path, 0)
    }

    /// Delete key from the desired platform-specific view of the registry.
    /// Key names are not case sensitive.
    ///
    /// # Examples
    /// ```no_run
    /// # use std::error::Error;
    /// # use winreg::RegKey;
    /// # use winreg::enums::*;
    /// # fn main() -> Result<(), Box<Error>> {
    /// // delete the key from the 32-bit registry view
    /// RegKey::predef(HKEY_LOCAL_MACHINE)
    ///     .delete_subkey_with_flags(r"Software\MyProduct\History", KEY_WOW64_32KEY)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn delete_subkey_with_flags<P: AsRef<OsStr>>(
        &self,
        path: P,
        perms: winapi_reg::REGSAM,
    ) -> io::Result<()> {
        let c_path = to_utf16(path);
        match unsafe {
            winapi_reg::RegDeleteKeyExW(
                self.hkey,
                c_path.as_ptr(), // This parameter cannot be NULL.
                perms,
                0,
            )
        } {
            0 => Ok(()),
            err => werr!(err),
        }
    }

    /// Part of `transactions` feature.
    #[cfg(feature = "transactions")]
    pub fn delete_subkey_transacted<P: AsRef<OsStr>>(
        &self,
        path: P,
        t: &Transaction,
    ) -> io::Result<()> {
        self.delete_subkey_transacted_with_flags(path, t, 0)
    }

    /// Part of `transactions` feature.
    #[cfg(feature = "transactions")]
    pub fn delete_subkey_transacted_with_flags<P: AsRef<OsStr>>(
        &self,
        path: P,
        t: &Transaction,
        perms: winapi_reg::REGSAM,
    ) -> io::Result<()> {
        let c_path = to_utf16(path);
        match unsafe {
            winapi_reg::RegDeleteKeyTransactedW(
                self.hkey,
                c_path.as_ptr(), // This parameter cannot be NULL.
                perms,
                0,
                t.handle,
                ptr::null_mut(),
            )
        } {
            0 => Ok(()),
            err => werr!(err),
        }
    }

    /// Recursively delete subkey with all its subkeys and values.
    /// If `path` is an empty string, the subkeys and values of this key are deleted.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// # use winreg::RegKey;
    /// # use winreg::enums::*;
    /// # fn main() -> Result<(), Box<Error>> {
    /// RegKey::predef(HKEY_CURRENT_USER)
    ///     .delete_subkey_all("Software\\MyProduct")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn delete_subkey_all<P: AsRef<OsStr>>(&self, path: P) -> io::Result<()> {
        let c_path;
        let path_ptr = if path.as_ref().is_empty() {
            ptr::null()
        } else {
            c_path = to_utf16(path);
            c_path.as_ptr()
        };
        match unsafe {
            winapi_reg::RegDeleteTreeW(
                self.hkey,
                path_ptr, //If this parameter is NULL, the subkeys and values of this key are deleted.
            ) as DWORD
        } {
            0 => Ok(()),
            err => werr!(err),
        }
    }

    /// Get a value from registry and seamlessly convert it to the specified rust type
    /// with `FromRegValue` implemented (currently `String`, `u32` and `u64`).
    /// Will get the `Default` value if `name` is an empty string.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// # use winreg::RegKey;
    /// # use winreg::enums::*;
    /// # fn main() -> Result<(), Box<Error>> {
    /// let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    /// let settings = hkcu.open_subkey("Software\\MyProduct\\Settings")?;
    /// let server: String = settings.get_value("server")?;
    /// let port: u32 = settings.get_value("port")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn get_value<T: FromRegValue, N: AsRef<OsStr>>(&self, name: N) -> io::Result<T> {
        match self.get_raw_value(name) {
            Ok(ref val) => FromRegValue::from_reg_value(val),
            Err(err) => Err(err),
        }
    }

    /// Get raw bytes from registry value.
    /// Will get the `Default` value if `name` is an empty string.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// # use winreg::RegKey;
    /// # use winreg::enums::*;
    /// # fn main() -> Result<(), Box<Error>> {
    /// let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    /// let settings = hkcu.open_subkey("Software\\MyProduct\\Settings")?;
    /// let data = settings.get_raw_value("data")?;
    /// println!("Bytes: {:?}", data.bytes);
    /// # Ok(())
    /// # }
    /// ```
    pub fn get_raw_value<N: AsRef<OsStr>>(&self, name: N) -> io::Result<RegValue> {
        let c_name = to_utf16(name);
        let mut buf_len: DWORD = 2048;
        let mut buf_type: DWORD = 0;
        let mut buf: Vec<u8> = Vec::with_capacity(buf_len as usize);
        loop {
            match unsafe {
                winapi_reg::RegQueryValueExW(
                    self.hkey,
                    c_name.as_ptr() as *const u16,
                    ptr::null_mut(),
                    &mut buf_type,
                    buf.as_mut_ptr() as LPBYTE,
                    &mut buf_len,
                ) as DWORD
            } {
                0 => {
                    unsafe {
                        buf.set_len(buf_len as usize);
                    }
                    // minimal check before transmute to RegType
                    if buf_type > winnt::REG_QWORD {
                        return werr!(winerror::ERROR_BAD_FILE_TYPE);
                    }
                    let t: RegType = unsafe { transmute(buf_type as u8) };
                    return Ok(RegValue {
                        bytes: buf,
                        vtype: t,
                    });
                }
                winerror::ERROR_MORE_DATA => {
                    buf.reserve(buf_len as usize);
                }
                err => return werr!(err),
            }
        }
    }

    /// Seamlessly convert a value from a rust type and write it to the registry value
    /// with `ToRegValue` trait implemented (currently `String`, `&str`, `u32` and `u64`).
    /// Will set the `Default` value if `name` is an empty string.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// # use winreg::RegKey;
    /// # use winreg::enums::*;
    /// # fn main() -> Result<(), Box<Error>> {
    /// let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    /// let (settings, disp) = hkcu.create_subkey("Software\\MyProduct\\Settings")?;
    /// settings.set_value("server", &"www.example.com")?;
    /// settings.set_value("port", &8080u32)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn set_value<T: ToRegValue, N: AsRef<OsStr>>(&self, name: N, value: &T) -> io::Result<()> {
        self.set_raw_value(name, &value.to_reg_value())
    }

    /// Write raw bytes from `RegValue` struct to a registry value.
    /// Will set the `Default` value if `name` is an empty string.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// use winreg::{RegKey, RegValue};
    /// use winreg::enums::*;
    /// # fn main() -> Result<(), Box<Error>> {
    /// let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    /// let settings = hkcu.open_subkey("Software\\MyProduct\\Settings")?;
    /// let bytes: Vec<u8> = vec![1, 2, 3, 5, 8, 13, 21, 34, 55, 89];
    /// let data = RegValue{ vtype: REG_BINARY, bytes: bytes};
    /// settings.set_raw_value("data", &data)?;
    /// println!("Bytes: {:?}", data.bytes);
    /// # Ok(())
    /// # }
    /// ```
    pub fn set_raw_value<N: AsRef<OsStr>>(&self, name: N, value: &RegValue) -> io::Result<()> {
        let c_name = to_utf16(name);
        let t = value.vtype.clone() as DWORD;
        match unsafe {
            winapi_reg::RegSetValueExW(
                self.hkey,
                c_name.as_ptr(),
                0,
                t,
                value.bytes.as_ptr() as *const BYTE,
                value.bytes.len() as u32,
            ) as DWORD
        } {
            0 => Ok(()),
            err => werr!(err),
        }
    }

    /// Delete specified value from registry.
    /// Will delete the `Default` value if `name` is an empty string.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// # use winreg::RegKey;
    /// # use winreg::enums::*;
    /// # fn main() -> Result<(), Box<Error>> {
    /// let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    /// let settings = hkcu.open_subkey("Software\\MyProduct\\Settings")?;
    /// settings.delete_value("data")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn delete_value<N: AsRef<OsStr>>(&self, name: N) -> io::Result<()> {
        let c_name = to_utf16(name);
        match unsafe { winapi_reg::RegDeleteValueW(self.hkey, c_name.as_ptr()) as DWORD } {
            0 => Ok(()),
            err => werr!(err),
        }
    }

    /// Save `Encodable` type to a registry key.
    /// Part of `serialization-serde` feature.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// #[macro_use]
    /// extern crate serde_derive;
    /// extern crate winreg;
    /// use winreg::RegKey;
    /// use winreg::enums::*;
    ///
    /// #[derive(Serialize)]
    /// struct Rectangle{
    ///     x: u32,
    ///     y: u32,
    ///     w: u32,
    ///     h: u32,
    /// }
    ///
    /// #[derive(Serialize)]
    /// struct Settings{
    ///     current_dir: String,
    ///     window_pos: Rectangle,
    ///     show_in_tray: bool,
    /// }
    ///
    /// # fn main() -> Result<(), Box<Error>> {
    /// let s: Settings = Settings{
    ///     current_dir: "C:\\".to_owned(),
    ///     window_pos: Rectangle{ x:200, y: 100, w: 800, h: 500 },
    ///     show_in_tray: false,
    /// };
    /// let s_key = RegKey::predef(HKEY_CURRENT_USER)
    ///     .open_subkey("Software\\MyProduct\\Settings")?;
    /// s_key.encode(&s)?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "serialization-serde")]
    pub fn encode<T: serde::Serialize>(&self, value: &T) -> encoder::EncodeResult<()> {
        let mut encoder = encoder::Encoder::from_key(self)?;
        value.serialize(&mut encoder)?;
        encoder.commit()
    }

    /// Load `Decodable` type from a registry key.
    /// Part of `serialization-serde` feature.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::error::Error;
    /// #[macro_use]
    /// extern crate serde_derive;
    /// extern crate winreg;
    /// use winreg::RegKey;
    /// use winreg::enums::*;
    ///
    /// #[derive(Deserialize)]
    /// struct Rectangle{
    ///     x: u32,
    ///     y: u32,
    ///     w: u32,
    ///     h: u32,
    /// }
    ///
    /// #[derive(Deserialize)]
    /// struct Settings{
    ///     current_dir: String,
    ///     window_pos: Rectangle,
    ///     show_in_tray: bool,
    /// }
    ///
    /// # fn main() -> Result<(), Box<Error>> {
    /// let s_key = RegKey::predef(HKEY_CURRENT_USER)
    ///     .open_subkey("Software\\MyProduct\\Settings")?;
    /// let s: Settings = s_key.decode()?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "serialization-serde")]
    pub fn decode<'de, T: serde::Deserialize<'de>>(&self) -> decoder::DecodeResult<T> {
        let mut decoder = decoder::Decoder::from_key(self)?;
        T::deserialize(&mut decoder)
    }

    fn close_(&mut self) -> io::Result<()> {
        // don't try to close predefined keys
        if self.hkey >= enums::HKEY_CLASSES_ROOT {
            return Ok(());
        };
        match unsafe { winapi_reg::RegCloseKey(self.hkey) as DWORD } {
            0 => Ok(()),
            err => werr!(err),
        }
    }

    fn enum_key(&self, index: DWORD) -> Option<io::Result<String>> {
        let mut name_len = 2048;
        let mut name = [0 as WCHAR; 2048];
        match unsafe {
            winapi_reg::RegEnumKeyExW(
                self.hkey,
                index,
                name.as_mut_ptr(),
                &mut name_len,
                ptr::null_mut(), // reserved
                ptr::null_mut(), // lpClass: LPWSTR,
                ptr::null_mut(), // lpcClass: LPDWORD,
                ptr::null_mut(), // lpftLastWriteTime: PFILETIME,
            ) as DWORD
        } {
            0 => match String::from_utf16(&name[..name_len as usize]) {
                Ok(s) => Some(Ok(s)),
                Err(_) => Some(werr!(winerror::ERROR_INVALID_BLOCK)),
            },
            winerror::ERROR_NO_MORE_ITEMS => None,
            err => Some(werr!(err)),
        }
    }

    fn enum_value(&self, index: DWORD) -> Option<io::Result<(String, RegValue)>> {
        let mut name_len = 2048;
        let mut name = [0 as WCHAR; 2048];

        let mut buf_len: DWORD = 2048;
        let mut buf_type: DWORD = 0;
        let mut buf: Vec<u8> = Vec::with_capacity(buf_len as usize);
        loop {
            match unsafe {
                winapi_reg::RegEnumValueW(
                    self.hkey,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    ptr::null_mut(), // reserved
                    &mut buf_type,
                    buf.as_mut_ptr() as LPBYTE,
                    &mut buf_len,
                ) as DWORD
            } {
                0 => {
                    let name = match String::from_utf16(&name[..name_len as usize]) {
                        Ok(s) => s,
                        Err(_) => return Some(werr!(winerror::ERROR_INVALID_DATA)),
                    };
                    unsafe {
                        buf.set_len(buf_len as usize);
                    }
                    // minimal check before transmute to RegType
                    if buf_type > winnt::REG_QWORD {
                        return Some(werr!(winerror::ERROR_BAD_FILE_TYPE));
                    }
                    let t: RegType = unsafe { transmute(buf_type as u8) };
                    let value = RegValue {
                        bytes: buf,
                        vtype: t,
                    };
                    return Some(Ok((name, value)));
                }
                winerror::ERROR_MORE_DATA => {
                    name_len += 1; //for NULL char
                    buf.reserve(buf_len as usize);
                }
                winerror::ERROR_NO_MORE_ITEMS => return None,
                err => return Some(werr!(err)),
            }
        }
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        self.close_().unwrap_or(());
    }
}

/// Iterator over subkeys names
pub struct EnumKeys<'key> {
    key: &'key RegKey,
    index: DWORD,
}

impl<'key> Iterator for EnumKeys<'key> {
    type Item = io::Result<String>;

    fn next(&mut self) -> Option<io::Result<String>> {
        match self.key.enum_key(self.index) {
            v @ Some(_) => {
                self.index += 1;
                v
            }
            e @ None => e,
        }
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.index += n as DWORD;
        self.next()
    }
}

/// Iterator over values
pub struct EnumValues<'key> {
    key: &'key RegKey,
    index: DWORD,
}

impl<'key> Iterator for EnumValues<'key> {
    type Item = io::Result<(String, RegValue)>;

    fn next(&mut self) -> Option<io::Result<(String, RegValue)>> {
        match self.key.enum_value(self.index) {
            v @ Some(_) => {
                self.index += 1;
                v
            }
            e @ None => e,
        }
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.index += n as DWORD;
        self.next()
    }
}

fn to_utf16<P: AsRef<OsStr>>(s: P) -> Vec<u16> {
    s.as_ref()
        .encode_wide()
        .chain(Some(0).into_iter())
        .collect()
}

fn v16_to_v8(v: &[u16]) -> Vec<u8> {
    unsafe { slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2).to_vec() }
}

#[cfg(all(test, feature = "serialization-serde"))]
#[macro_use]
extern crate serde_derive;

#[cfg(test)]
#[cfg_attr(feature = "clippy", allow(option_unwrap_used))]
#[cfg_attr(feature = "clippy", allow(result_unwrap_used))]
mod test {
    extern crate rand;
    use self::rand::Rng;
    use super::*;
    use std::collections::HashMap;
    use std::ffi::{OsStr, OsString};

    #[test]
    fn test_raw_handle() {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let handle = hklm.raw_handle();
        assert_eq!(HKEY_LOCAL_MACHINE, handle);
    }

    #[test]
    fn test_open_subkey_with_flags_query_info() {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let win = hklm
            .open_subkey_with_flags("Software\\Microsoft\\Windows", KEY_READ)
            .unwrap();

        let info = win.query_info().unwrap();
        info.get_last_write_time_system();
        #[cfg(feature = "chrono")]
        info.get_last_write_time_chrono();

        assert!(win
            .open_subkey_with_flags("CurrentVersion\\", KEY_READ)
            .is_ok());
        assert!(hklm
            .open_subkey_with_flags("i\\just\\hope\\nobody\\created\\that\\key", KEY_READ)
            .is_err());
    }

    #[test]
    fn test_create_subkey_disposition() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let path = "Software\\WinRegRsTestCreateSubkey";
        let (_subkey, disp) = hkcu.create_subkey(path).unwrap();
        assert_eq!(disp, REG_CREATED_NEW_KEY);
        let (_subkey2, disp2) = hkcu.create_subkey(path).unwrap();
        assert_eq!(disp2, REG_OPENED_EXISTING_KEY);
        hkcu.delete_subkey_all(&path).unwrap();
    }

    macro_rules! with_key {
        ($k:ident, $path:expr => $b:block) => {{
            let mut path = "Software\\WinRegRsTest".to_owned();
            path.push_str($path);
            let ($k, _disp) = RegKey::predef(HKEY_CURRENT_USER)
                .create_subkey(&path).unwrap();
            $b
            RegKey::predef(HKEY_CURRENT_USER)
            .delete_subkey_all(path).unwrap();
        }}
    }

    #[test]
    fn test_delete_subkey() {
        let path = "Software\\WinRegRsTestDeleteSubkey";
        RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(path)
            .unwrap();
        assert!(RegKey::predef(HKEY_CURRENT_USER)
            .delete_subkey(path)
            .is_ok());
    }

    #[test]
    fn test_delete_subkey_with_flags() {
        let path = "Software\\Classes\\WinRegRsTestDeleteSubkeyWithFlags";
        RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey_with_flags(path, KEY_WOW64_32KEY)
            .unwrap();
        assert!(RegKey::predef(HKEY_CURRENT_USER)
            .delete_subkey_with_flags(path, KEY_WOW64_32KEY)
            .is_ok());
    }

    #[test]
    fn test_copy_tree() {
        with_key!(key, "CopyTree" => {
            let (sub_tree, _sub_tree_disp) = key.create_subkey("Src\\Sub\\Tree").unwrap();
            for v in &["one", "two", "three"] {
                sub_tree.set_value(v, v).unwrap();
            }
            let (dst, _dst_disp) = key.create_subkey("Dst").unwrap();
            assert!(key.copy_tree("Src", &dst).is_ok());
        });
    }

    #[test]
    fn test_long_value() {
        with_key!(key, "LongValue" => {
            let name = "RustLongVal";
            let val1 = RegValue { vtype: REG_BINARY, bytes: (0..6000).map(|_| rand::random::<u8>()).collect() };
            key.set_raw_value(name, &val1).unwrap();
            let val2 = key.get_raw_value(name).unwrap();
            assert_eq!(val1, val2);
        });
    }

    #[test]
    fn test_string_value() {
        with_key!(key, "StringValue" => {
            let name = "RustStringVal";
            let val1 = "Test123 \n$%^&|+-*/\\()".to_owned();
            key.set_value(name, &val1).unwrap();
            let val2: String = key.get_value(name).unwrap();
            assert_eq!(val1, val2);
        });
    }

    #[test]
    fn test_long_string_value() {
        with_key!(key, "LongStringValue" => {
            let name = "RustLongStringVal";
            let val1 : String = rand::thread_rng().gen_ascii_chars().take(7000).collect();
            key.set_value(name, &val1).unwrap();
            let val2: String = key.get_value(name).unwrap();
            assert_eq!(val1, val2);
        });
    }

    #[test]
    fn test_os_string_value() {
        with_key!(key, "OsStringValue" => {
            let name = "RustOsStringVal";
            let val1 = OsStr::new("Test123 \n$%^&|+-*/\\()\u{0}");
            key.set_value(name, &val1).unwrap();
            let val2: OsString = key.get_value(name).unwrap();
            assert_eq!(val1, val2);
        });
    }

    #[test]
    fn test_long_os_string_value() {
        with_key!(key, "LongOsStringValue" => {
        let name = "RustLongOsStringVal";
        let mut val1 = rand::thread_rng().gen_ascii_chars().take(7000).collect::<String>();
        val1.push('\u{0}');
        let val1 = OsStr::new(&val1);
        key.set_value(name, &val1).unwrap();
        let val2: OsString = key.get_value(name).unwrap();
        assert_eq!(val1, val2);
        });
    }

    #[test]
    fn test_u32_value() {
        with_key!(key, "U32Value" => {
            let name = "RustU32Val";
            let val1 = 1_234_567_890u32;
            key.set_value(name, &val1).unwrap();
            let val2: u32 = key.get_value(name).unwrap();
            assert_eq!(val1, val2);
        });
    }

    #[test]
    fn test_u64_value() {
        with_key!(key, "U64Value" => {
            let name = "RustU64Val";
            let val1 = 1_234_567_891_011_121_314u64;
            key.set_value(name, &val1).unwrap();
            let val2: u64 = key.get_value(name).unwrap();
            assert_eq!(val1, val2);
        });
    }

    #[test]
    fn test_delete_value() {
        with_key!(key, "DeleteValue" => {
            let name = "WinregRsTestVal";
            key.set_value(name, &"Qwerty123").unwrap();
            assert!(key.delete_value(name).is_ok());
        });
    }

    #[test]
    fn test_enum_keys() {
        with_key!(key, "EnumKeys" => {
            let mut keys1 = vec!("qwerty", "asdf", "1", "2", "3", "5", "8", "йцукен");
            keys1.sort();
            for i in &keys1 {
                key.create_subkey(i).unwrap();
            }
            let keys2: Vec<_> = key.enum_keys().map(|x| x.unwrap()).collect();
            assert_eq!(keys1, keys2);
        });
    }

    #[test]
    fn test_enum_values() {
        with_key!(key, "EnumValues" => {
            let mut vals1 = vec!("qwerty", "asdf", "1", "2", "3", "5", "8", "йцукен");
            vals1.sort();
            for i in &vals1 {
                key.set_value(i,i).unwrap();
            }
            let mut vals2: Vec<String> = Vec::with_capacity(vals1.len());
            let mut vals3: Vec<String> = Vec::with_capacity(vals1.len());
            for (name, val) in key.enum_values()
                .map(|x| x.unwrap())
            {
                vals2.push(name);
                vals3.push(String::from_reg_value(&val).unwrap());
            }
            assert_eq!(vals1, vals2);
            assert_eq!(vals1, vals3);
        });
    }

    #[test]
    fn test_enum_long_values() {
        with_key!(key, "EnumLongValues" => {
            let mut vals = HashMap::with_capacity(3);

            for i in &[5500, 9500, 15000] {
                let name: String = format!("val{}", i);
                let val = RegValue { vtype: REG_BINARY, bytes: (0..*i).map(|_| rand::random::<u8>()).collect() };
                vals.insert(name, val);
            }

            for (name, val) in key.enum_values()
                                  .map(|x| x.unwrap())
            {
                assert_eq!(val.bytes, vals[&name].bytes);
            }
        });
    }

    #[cfg(feature = "serialization-serde")]
    #[test]
    fn test_serialization() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Rectangle {
            x: u32,
            y: u32,
            w: u32,
            h: u32,
        }

        #[derive(Debug, PartialEq, Serialize, Deserialize)]
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

        let v1 = Test {
            t_bool: false,
            t_u8: 127,
            t_u16: 32768,
            t_u32: 123_456_789,
            t_u64: 123_456_789_101_112,
            t_usize: 1_234_567_891,
            t_struct: Rectangle {
                x: 55,
                y: 77,
                w: 500,
                h: 300,
            },
            t_string: "Test123 \n$%^&|+-*/\\()".to_owned(),
            t_i8: -123,
            t_i16: -2049,
            t_i32: 20100,
            t_i64: -12_345_678_910,
            t_isize: -1_234_567_890,
            t_f64: -0.01,
            t_f32: 3.15,
        };

        with_key!(key, "Serialization" => {
            key.encode(&v1).unwrap();
            let v2: Test = key.decode().unwrap();
            assert_eq!(v1, v2);
        });
    }
}
