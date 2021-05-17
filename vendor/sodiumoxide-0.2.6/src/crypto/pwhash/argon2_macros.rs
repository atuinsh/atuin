macro_rules! argon2_module (($pwhash_name:ident,
                           $pwhash_str_name:ident,
                           $pwhash_str_verify_name:ident,
                           $saltbytes:expr,
                           $hashedpasswordbytes:expr,
                           $strprefix:expr,
                           $opslimit_interative:expr,
                           $opslimit_moderate:expr,
                           $opslimit_sensitive:expr,
                           $memlimit_interative:expr,
                           $memlimit_moderate:expr,
                           $memlimit_sensitive:expr,
                           $variant:expr) => (

use libc::{c_int, c_ulonglong};
use randombytes::randombytes_into;

/// Number of bytes in a `Salt`.
pub const SALTBYTES: usize = $saltbytes;

/// Number of bytes in a `HashedPassword`.
pub const HASHEDPASSWORDBYTES: usize = $hashedpasswordbytes;

/// All `HashedPasswords` start with this string.
pub const STRPREFIX: &'static [u8] = $strprefix;

/// Safe base line for `OpsLimit` for interactive password hashing.
pub const OPSLIMIT_INTERACTIVE: OpsLimit = OpsLimit($opslimit_interative);

/// Safe base line for `MemLimit` for interactive password hashing.
pub const MEMLIMIT_INTERACTIVE: MemLimit = MemLimit($memlimit_interative);

/// `OpsLimit` for moderately sensitive data.
pub const OPSLIMIT_MODERATE: OpsLimit = OpsLimit($opslimit_moderate);

/// `MemLimit` for moderately sensitive data.
pub const MEMLIMIT_MODERATE: MemLimit = MemLimit($memlimit_moderate);

/// `OpsLimit` for highly sensitive data.
pub const OPSLIMIT_SENSITIVE: OpsLimit = OpsLimit($opslimit_sensitive);

/// `MemLimit` for highly sensitive data.
pub const MEMLIMIT_SENSITIVE: MemLimit = MemLimit($memlimit_sensitive);

/// Variant id for the Argon2i13 algorithm
pub const VARIANT: u32 = $variant;

/// `OpsLimit` represents the maximum number of computations to perform when
/// using the functions in this module.
///
/// A high `OpsLimit` will make the functions
/// require more CPU cycles
#[derive(Copy, Clone, Debug)]
pub struct OpsLimit(pub usize);

/// `MemLimit` represents the maximum amount of RAM that the functions in this
/// module will use, in bytes.
///
/// It is highly recommended to allow the functions to use
/// at least 16 megabytes.
#[derive(Copy, Clone, Debug)]
pub struct MemLimit(pub usize);

new_type! {
    /// `Salt` used for password hashing
    public Salt(SALTBYTES);
}

new_type! {
    /// `HashedPassword`is a password verifier generated from a password
    ///
    /// A `HashedPassword` is zero-terminated, includes only ASCII characters and can
    /// be conveniently stored into SQL databases and other data stores. No
    /// additional information has to be stored in order to verify the password.
    public HashedPassword(HASHEDPASSWORDBYTES);
}

/// `gen_salt()` randomly generates a new `Salt` for key derivation
///
/// THREAD SAFETY: `gen_salt()` is thread-safe provided that you have called
/// `sodiumoxide::init()` once before using any other function from sodiumoxide.
pub fn gen_salt() -> Salt {
    let mut salt = Salt([0; SALTBYTES]);
    randombytes_into(&mut salt.0);
    salt
}

/// The `derive_key()` function derives a key from a password and a `Salt`
///
/// The computed key is stored into key.
///
/// `opslimit` represents a maximum amount of computations to perform. Raising
/// this number will make the function require more CPU cycles to compute a key.
///
/// `memlimit` is the maximum amount of RAM that the function will use, in
/// bytes. It is highly recommended to allow the function to use at least 16
/// megabytes.
///
/// For interactive, online operations, `OPSLIMIT_INTERACTIVE` and
/// `MEMLIMIT_INTERACTIVE` provide a safe base line for these two
/// parameters. However, using higher values may improve security.
///
/// For highly sensitive data, `OPSLIMIT_SENSITIVE` and `MEMLIMIT_SENSITIVE` can
/// be used as an alternative. But with these parameters, deriving a key takes
/// more than 10 seconds on a 2.8 Ghz Core i7 CPU and requires up to 1 gigabyte
/// of dedicated RAM.
///
/// The salt should be unpredictable. `gen_salt()` is the easiest way to create a `Salt`.
///
/// Keep in mind that in order to produce the same key from the same password,
/// the same salt, and the same values for opslimit and memlimit have to be
/// used.
///
/// The function returns `Ok(key)` on success and `Err(())` if the computation didn't
/// complete, usually because the operating system refused to allocate the
/// amount of requested memory.
pub fn derive_key<'a>(
    key: &'a mut [u8],
    passwd: &[u8],
    &Salt(ref sb): &Salt,
    OpsLimit(opslimit): OpsLimit,
    MemLimit(memlimit): MemLimit,
) -> Result<&'a [u8], ()> {

    let res = unsafe {
        $pwhash_name(
            key.as_mut_ptr(),
            key.len() as c_ulonglong,
            passwd.as_ptr() as *const _,
            passwd.len() as c_ulonglong,
            sb as *const _,
            opslimit as c_ulonglong,
            memlimit,
            VARIANT as c_int)
    };

    match res {
        0 => Ok(key),
        _ => Err(()),
    }
}


/// The `pwhash()` returns a `HashedPassword` which
/// includes:
///
/// - the result of a memory-hard, CPU-intensive hash function applied to the password
///   `passwd`
/// - the automatically generated salt used for the
///   previous computation
/// - the other parameters required to verify the password: opslimit and memlimit
///
/// `OPSLIMIT_INTERACTIVE` and `MEMLIMIT_INTERACTIVE` are safe baseline
/// values to use for `opslimit` and `memlimit`.
///
/// The function returns `Ok(hashed_password)` on success and `Err(())` if it didn't complete
/// successfully
pub fn pwhash(
    passwd: &[u8],
    OpsLimit(opslimit): OpsLimit,
    MemLimit(memlimit): MemLimit,
) -> Result<HashedPassword, ()> {
    let mut out = HashedPassword([0; HASHEDPASSWORDBYTES]);
    let res = unsafe {
        $pwhash_str_name(
            out.0.as_mut_ptr() as *mut _,
            passwd.as_ptr() as *const _,
            passwd.len() as c_ulonglong,
            opslimit as c_ulonglong,
            memlimit)
    };

    match res {
        0 => Ok(out),
        _ => Err(()),
    }
}

/// `pwhash_verify()` verifies that the password `str_` is a valid password
/// verification string (as generated by `pwhash()`) for `passwd`
///
/// It returns `true` if the verification succeeds, and `false` on error.
pub fn pwhash_verify(hp: &HashedPassword, passwd: &[u8]) -> bool {
    let res = unsafe {
        $pwhash_str_verify_name(
            hp.0.as_ptr() as *const _,
            passwd.as_ptr() as *const _,
            passwd.len() as c_ulonglong)
    };

    res == 0
}

));
