//! `redox-users` is designed to be a small, low-ish level interface
//! to system user and group information, as well as user password
//! authentication. It is OS-specific and will break horribly on platforms
//! that are not [Redox-OS](https://redox-os.org).
//!
//! # Permissions
//! Because this is a system level tool dealing with password
//! authentication, programs are often required to run with
//! escalated priveleges. The implementation of the crate is
//! privelege unaware. The only privelege requirements are those
//! laid down by the system administrator over these files:
//! - `/etc/group`
//!   - Read: Required to access group information
//!   - Write: Required to change group information
//! - `/etc/passwd`
//!   - Read: Required to access user information
//!   - Write: Required to change user information
//! - `/etc/shadow`
//!   - Read: Required to authenticate users
//!   - Write: Required to set user passwords
//!
//! # Reimplementation
//! This crate is designed to be as small as possible without
//! sacrificing critical functionality. The idea is that a small
//! enough redox-users will allow easy re-implementation based on
//! the same flexible API. This would allow more complicated authentication
//! schemes for redox in future without breakage of existing
//! software.

use std::convert::From;
use std::error::Error;
use std::fmt::{self, Debug};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::marker::PhantomData;
#[cfg(target_os = "redox")]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(not(target_os = "redox"))]
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::slice::{Iter, IterMut};
#[cfg(not(test))]
#[cfg(feature = "auth")]
use std::thread;
use std::time::Duration;

//#[cfg(not(target_os = "redox"))]
//use nix::fcntl::{flock, FlockArg};

#[cfg(target_os = "redox")]
use syscall::flag::{O_EXLOCK, O_SHLOCK};
use syscall::Error as SyscallError;

const PASSWD_FILE: &'static str = "/etc/passwd";
const GROUP_FILE: &'static str = "/etc/group";
#[cfg(feature = "auth")]
const SHADOW_FILE: &'static str = "/etc/shadow";

#[cfg(target_os = "redox")]
const DEFAULT_SCHEME: &'static str = "file:";
#[cfg(not(target_os = "redox"))]
const DEFAULT_SCHEME: &'static str = "";

const MIN_ID: usize = 1000;
const MAX_ID: usize = 6000;
const DEFAULT_TIMEOUT: u64 = 3;

#[cfg(feature = "auth")]
const USER_AUTH_FULL_EXPECTED_HASH: &str = "A User<auth::Full> had no hash";

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// Errors that might happen while using this crate
#[derive(Debug, PartialEq)]
pub enum UsersError {
    Os { reason: String },
    Parsing { reason: String, line: usize },
    NotFound,
    AlreadyExists
}

impl fmt::Display for UsersError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UsersError::Os { reason } => write!(f, "os error: code {}", reason),
            UsersError::Parsing { reason, line } => {
                write!(f, "parse error line {}: {}", line, reason)
            },
            UsersError::NotFound => write!(f, "user/group not found"),
            UsersError::AlreadyExists => write!(f, "user/group already exists")
        }
    }
}

impl Error for UsersError {
    fn description(&self) -> &str { "UsersError" }

    fn cause(&self) -> Option<&dyn Error> { None }
}

#[inline]
fn parse_error(line: usize, reason: &str) -> UsersError {
    UsersError::Parsing {
        reason: reason.into(),
        line,
    }
}

#[inline]
fn os_error(reason: &str) -> UsersError {
    UsersError::Os {
        reason: reason.into()
    }
}

impl From<SyscallError> for UsersError {
    fn from(syscall_error: SyscallError) -> UsersError {
        UsersError::Os {
            reason: format!("{}", syscall_error)
        }
    }
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
enum Lock {
    Shared,
    Exclusive,
}

impl Lock {
    #[cfg(target_os = "redox")]
    fn as_olock(self) -> i32 {
        (match self {
            Lock::Shared => O_SHLOCK,
            Lock::Exclusive => O_EXLOCK,
        }) as i32
    }
    
    /*#[cfg(not(target_os = "redox"))]
    fn as_flock(self) -> FlockArg {
        match self {
            Lock::Shared => FlockArg::LockShared,
            Lock::Exclusive => FlockArg::LockExclusive,
        }
    }*/
}

/// Naive semi-cross platform file locking (need to support linux for tests).
#[allow(dead_code)]
fn locked_file(file: impl AsRef<Path>, _lock: Lock) -> Result<File> {
    #[cfg(test)]
    println!("Open file: {}", file.as_ref().display());

    #[cfg(target_os = "redox")]
    {
        Ok(OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(_lock.as_olock())
            .open(file)?)
    }
    #[cfg(not(target_os = "redox"))]
    #[cfg_attr(rustfmt, rustfmt_skip)]
    {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(file)?;
        let fd = file.as_raw_fd();
        eprintln!("Fd: {}", fd);
        //flock(fd, _lock.as_flock())?;
        Ok(file)
    }
}

/// Reset a file for rewriting (user/group dbs must be erased before write-out)
fn reset_file(fd: &mut File) -> Result<()> {
    fd.set_len(0)?;
    fd.seek(SeekFrom::Start(0))?;
    Ok(())
}

/// Marker types for [`User`] and [`AllUsers`].
pub mod auth {
    /// Marker type indicating that a `User` only has access to world-readable
    /// user information, and cannot authenticate.
    #[derive(Debug)]
    pub struct Basic {}
    
    /// Marker type indicating that a `User` has access to all user
    /// information, including password hashes.
    #[cfg(feature = "auth")]
    #[derive(Debug)]
    pub struct Full {}
}

/// A struct representing a Redox user.
/// Currently maps to an entry in the `/etc/passwd` file.
///
/// `A` should be a type from [`crate::auth`].
///
/// # Unset vs. Blank Passwords
/// A note on unset passwords vs. blank passwords. A blank password
/// is a hash field that is completely blank (aka, `""`). According
/// to this crate, successful login is only allowed if the input
/// password is blank as well.
///
/// An unset password is one whose hash is not empty (`""`), but
/// also not a valid serialized argon2rs hashing session. This
/// hash always returns `false` upon attempted verification. The
/// most commonly used hash for an unset password is `"!"`, but
/// this crate makes no distinction. The most common way to unset
/// the password is to use [`User::unset_passwd`].
pub struct User<A> {
    /// Username (login name)
    pub user: String,
    /// User id
    pub uid: usize,
    /// Group id
    pub gid: usize,
    /// Real name (human readable, can contain spaces)
    pub name: String,
    /// Home directory path
    pub home: String,
    /// Shell path
    pub shell: String,
    
    // Stored password hash text and an indicator to determine if the text is a
    // hash.
    #[cfg(feature = "auth")]
    hash: Option<(String, bool)>,
    // Failed login delay duration
    auth_delay: Duration,
    auth: PhantomData<A>,
}

impl<A> User<A> {
    /// Get a Command to run the user's default shell (see [`User::login_cmd`]
    /// for more docs).
    pub fn shell_cmd(&self) -> Command { self.login_cmd(&self.shell) }

    /// Provide a login command for the user, which is any entry point for
    /// starting a user's session, whether a shell (use [`User::shell_cmd`]
    /// instead) or a graphical init.
    ///
    /// The `Command` will use the user's `uid` and `gid`, its `current_dir`
    /// will be set to the user's home directory, and the follwing enviroment
    /// variables will be populated:
    ///
    ///    - `USER` set to the user's `user` field.
    ///    - `UID` set to the user's `uid` field.
    ///    - `GROUPS` set the user's `gid` field.
    ///    - `HOME` set to the user's `home` field.
    ///    - `SHELL` set to the user's `shell` field.
    pub fn login_cmd<T>(&self, cmd: T) -> Command
        where T: std::convert::AsRef<std::ffi::OsStr> + AsRef<str>
    {
        let mut command = Command::new(cmd);
        command
            .uid(self.uid as u32)
            .gid(self.gid as u32)
            .current_dir(&self.home)
            .env("USER", &self.user)
            .env("UID", format!("{}", self.uid))
            .env("GROUPS", format!("{}", self.gid))
            .env("HOME", &self.home)
            .env("SHELL", &self.shell);
        command
    }
    
    fn from_passwd_entry(s: &str, line: usize) -> Result<Self> {
        let mut parts = s.split(';');

        let user = parts
            .next()
            .ok_or(parse_error(line, "expected user"))?;
        let uid = parts
            .next()
            .ok_or(parse_error(line, "expected uid"))?
            .parse::<usize>()?;
        let gid = parts
            .next()
            .ok_or(parse_error(line, "expected uid"))?
            .parse::<usize>()?;
        let name = parts
            .next()
            .ok_or(parse_error(line, "expected real name"))?;
        let home = parts
            .next()
            .ok_or(parse_error(line, "expected home dir path"))?;
        let shell = parts
            .next()
            .ok_or(parse_error(line, "expected shell path"))?;

        Ok(User::<A> {
            user: user.into(),
            uid,
            gid,
            name: name.into(),
            home: home.into(),
            shell: shell.into(),
            #[cfg(feature = "auth")]
            hash: None,
            auth: PhantomData,
            auth_delay: Duration::default(),
        })
    }

    /// Format this user as an entry in `/etc/passwd`.
    fn passwd_entry(&self) -> String {
        #[cfg_attr(rustfmt, rustfmt_skip)]
        format!("{};{};{};{};{};{}\n",
            self.user, self.uid, self.gid, self.name, self.home, self.shell
        )
    }
}

/// Additional methods for if this `User` is authenticatable.
#[cfg(feature = "auth")]
impl User<auth::Full> {
    /// Set the password for a user. Make **sure** that `password`
    /// is actually what the user wants as their password (this doesn't).
    ///
    /// To set the password blank, pass `""` as `password`.
    pub fn set_passwd(&mut self, password: impl AsRef<str>) -> Result<()> {
        let password = password.as_ref();

        self.hash = if password != "" {
            let mut buf = [0u8; 8];
            getrandom::getrandom(&mut buf)?;
            let salt = format!("{:X}", u64::from_ne_bytes(buf));
            let config = argon2::Config::default();
            let hash = argon2::hash_encoded(
                password.as_bytes(),
                salt.as_bytes(),
                &config
            )?;
            Some((hash, true))
        } else {
            Some(("".into(), false))
        };
        Ok(())
    }

    /// Unset the password ([`User::verify_passwd`] always returns `false`).
    pub fn unset_passwd(&mut self) {
        self.hash = Some(("!".into(), false));
    }

    /// Verify the password. If the hash is empty, this only returns `true` if
    /// `password` is also empty.
    ///
    /// Note that this is a blocking operation if the password is incorrect.
    /// See [`Config::auth_delay`] to set the wait time. Default is 3 seconds.
    pub fn verify_passwd(&self, password: impl AsRef<str>) -> bool {
        let &(ref hash, ref encoded) = self.hash.as_ref()
            .expect(USER_AUTH_FULL_EXPECTED_HASH);
        let password = password.as_ref();

        let verified = if *encoded {
            argon2::verify_encoded(&hash, password.as_bytes()).unwrap()
        } else {
            hash == "" && password == ""
        };

        if !verified {
            #[cfg(not(test))] // Make tests run faster
            thread::sleep(self.auth_delay);
        }
        verified
    }

    /// Determine if the hash for the password is blank ([`User::verify_passwd`]
    /// returns `true` *only* when the password is blank).
    pub fn is_passwd_blank(&self) -> bool {
        let &(ref hash, ref encoded) = self.hash.as_ref()
            .expect(USER_AUTH_FULL_EXPECTED_HASH);
        hash == "" && ! encoded
    }

    /// Determine if the hash for the password is unset
    /// ([`User::verify_passwd`] returns `false` regardless of input).
    pub fn is_passwd_unset(&self) -> bool {
        let &(ref hash, ref encoded) = self.hash.as_ref()
            .expect(USER_AUTH_FULL_EXPECTED_HASH);
        hash != "" && ! encoded
    }

    fn shadow_entry(&self) -> String {
        let hashstring = match self.hash {
            Some((ref hash, _)) => hash,
            None => panic!(USER_AUTH_FULL_EXPECTED_HASH)
        };
        format!("{};{}\n", self.user, hashstring)
    }

    /// Give this a hash string (not a shadowfile entry!!!)
    fn populate_hash(&mut self, hash: &str) -> Result<()> {
        let encoded = match hash {
            "" => false,
            "!" => false,
            _ => true,
        };
        self.hash = Some((hash.to_string(), encoded));
        Ok(())
    }
}

impl<A> Name for User<A> {
    fn name(&self) -> &str {
        &self.user
    }
}

impl<A> Id for User<A> {
    fn id(&self) -> usize {
        self.uid
    }
}

impl<A> Debug for User<A> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("User")
            .field("user", &self.user)
            .field("uid", &self.uid)
            .field("gid", &self.gid)
            .field("name", &self.name)
            .field("home", &self.home)
            .field("shell", &self.shell)
            .field("auth_delay", &self.auth_delay)
            .finish()
    }
}

/// A struct representing a Redox user group.
/// Currently maps to an `/etc/group` file entry.
#[derive(Debug)]
pub struct Group {
    /// Group name
    pub group: String,
    /// Unique group id
    pub gid: usize,
    /// Group members' usernames
    pub users: Vec<String>,
}

impl Group {
    fn from_group_entry(s: &str, line: usize) -> Result<Self> {
        let mut parts = s.trim()
            .split(';');

        let group = parts
            .next()
            .ok_or(parse_error(line, "expected group"))?;
        let gid = parts
            .next()
            .ok_or(parse_error(line, "expected gid"))?
            .parse::<usize>()?;
        let users_str = parts.next()
            .unwrap_or("");
        let users = users_str.split(',')
            .filter_map(|u| if u == "" {
                None
            } else {
                Some(u.into())
            })
            .collect();

        Ok(Group {
            group: group.into(),
            gid,
            users,
        })
    }

    /// Format this group as an entry in `/etc/group`. This
    /// is an implementation detail, do NOT rely on this trait
    /// being implemented in future.
    fn group_entry(&self) -> String {
        #[cfg_attr(rustfmt, rustfmt_skip)]
        format!("{};{};{}\n",
            self.group,
            self.gid,
            self.users.join(",").trim_matches(',')
        )
    }
}

impl Name for Group {
    fn name(&self) -> &str {
        &self.group
    }
}

impl Id for Group {
    fn id(&self) -> usize {
        self.gid
    }
}

/// Gets the current process effective user ID.
///
/// This function issues the `geteuid` system call returning the process effective
/// user id.
///
/// # Examples
///
/// Basic usage:
///
/// ```no_run
/// # use redox_users::get_euid;
/// let euid = get_euid().unwrap();
/// ```
pub fn get_euid() -> Result<usize> {
    match syscall::geteuid() {
        Ok(euid) => Ok(euid),
        Err(syscall_error) => Err(From::from(os_error(syscall_error.text())))
    }
}

/// Gets the current process real user ID.
///
/// This function issues the `getuid` system call returning the process real
/// user id.
///
/// # Examples
///
/// Basic usage:
///
/// ```no_run
/// # use redox_users::get_uid;
/// let uid = get_uid().unwrap();
/// ```
pub fn get_uid() -> Result<usize> {
    match syscall::getuid() {
        Ok(uid) => Ok(uid),
        Err(syscall_error) => Err(From::from(os_error(syscall_error.text())))
    }
}

/// Gets the current process effective group ID.
///
/// This function issues the `getegid` system call returning the process effective
/// group id.
///
/// # Examples
///
/// Basic usage:
///
/// ```no_run
/// # use redox_users::get_egid;
/// let egid = get_egid().unwrap();
/// ```
pub fn get_egid() -> Result<usize> {
    match syscall::getegid() {
        Ok(egid) => Ok(egid),
        Err(syscall_error) => Err(From::from(os_error(syscall_error.text())))
    }
}

/// Gets the current process real group ID.
///
/// This function issues the `getegid` system call returning the process real
/// group id.
///
/// # Examples
///
/// Basic usage:
///
/// ```no_run
/// # use redox_users::get_gid;
/// let gid = get_gid().unwrap();
/// ```
pub fn get_gid() -> Result<usize> {
    match syscall::getgid() {
        Ok(gid) => Ok(gid),
        Err(syscall_error) => Err(From::from(os_error(syscall_error.text())))
    }
}

/// A generic configuration that allows fine control of an [`AllUsers`] or
/// [`AllGroups`].
///
/// `auth_delay` is not used by [`AllGroups`]
///
/// In most situations, [`Config::default`](struct.Config.html#impl-Default)
/// will work just fine. The other fields are for finer control if it is
/// required.
///
/// # Example
/// ```
/// # use redox_users::Config;
/// use std::time::Duration;
///
/// let cfg = Config::default()
///     .min_id(500)
///     .max_id(1000)
///     .auth_delay(Duration::from_secs(5));
/// ```
#[derive(Clone, Debug)]
pub struct Config {
    scheme: String,
    auth_delay: Duration,
    min_id: usize,
    max_id: usize,
}

impl Config {
    /// Set the delay for a failed authentication. Default is 3 seconds.
    pub fn auth_delay(mut self, delay: Duration) -> Config {
        self.auth_delay = delay;
        self
    }

    /// Set the smallest ID possible to use when finding an unused ID.
    pub fn min_id(mut self, id: usize) -> Config {
        self.min_id = id;
        self
    }

    /// Set the largest possible ID to use when finding an unused ID.
    pub fn max_id(mut self, id: usize) -> Config {
        self.max_id = id;
        self
    }

    /// Set the scheme relative to which the [`AllUsers`] or [`AllGroups`]
    /// should be looking for its data files. This is a compromise between
    /// exposing implementation details and providing fine enough
    /// control over the behavior of this API.
    pub fn scheme(mut self, scheme: String) -> Config {
        self.scheme = scheme;
        self
    }

    // Prepend a path with the scheme in this Config
    fn in_scheme(&self, path: impl AsRef<Path>) -> PathBuf {
        let mut canonical_path = PathBuf::from(&self.scheme);
        // Should be a little careful here, not sure I want this behavior
        if path.as_ref().is_absolute() {
            // This is nasty
            canonical_path.push(path.as_ref().to_string_lossy()[1..].to_string());
        } else {
            canonical_path.push(path);
        }
        canonical_path
    }
}

impl Default for Config {
    /// The default base scheme is `file:`.
    ///
    /// The default auth delay is 3 seconds.
    ///
    /// The default min and max ids are 1000 and 6000.
    fn default() -> Config {
        Config {
            scheme: String::from(DEFAULT_SCHEME),
            auth_delay: Duration::new(DEFAULT_TIMEOUT, 0),
            min_id: MIN_ID,
            max_id: MAX_ID,
        }
    }
}

// Nasty hack to prevent the compiler complaining about
// "leaking" `AllInner`
mod sealed {
    use crate::Config;

    pub trait Name {
        fn name(&self) -> &str;
    }

    pub trait Id {
        fn id(&self) -> usize;
    }

    pub trait AllInner {
        // Group+User, thanks Dad
        type Gruser: Name + Id;

        /// These functions grab internal elements so that the other
        /// methods of `All` can manipulate them.
        fn list(&self) -> &Vec<Self::Gruser>;
        fn list_mut(&mut self) -> &mut Vec<Self::Gruser>;
        fn config(&self) -> &Config;
    }
}

use sealed::{AllInner, Id, Name};

/// This trait is used to remove repetitive API items from
/// [`AllGroups`] and [`AllUsers`]. It uses a hidden trait
/// so that the implementations of functions can be implemented
/// at the trait level. Do not try to implement this trait.
pub trait All: AllInner {
    /// Get an iterator borrowing all [`User`]s or [`Group`]s on the system.
    fn iter(&self) -> Iter<<Self as AllInner>::Gruser> {
        self.list().iter()
    }

    /// Get an iterator mutably borrowing all [`User`]s or [`Group`]s on the
    /// system.
    fn iter_mut(&mut self) -> IterMut<<Self as AllInner>::Gruser> {
        self.list_mut().iter_mut()
    }

    /// Borrow the [`User`] or [`Group`] with a given name.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```no_run
    /// # use redox_users::{All, AllUsers, Config};
    /// let users = AllUsers::basic(Config::default()).unwrap();
    /// let user = users.get_by_name("root").unwrap();
    /// ```
    fn get_by_name(&self, name: impl AsRef<str>) -> Option<&<Self as AllInner>::Gruser> {
        self.iter()
            .find(|gruser| gruser.name() == name.as_ref() )
    }

    /// Mutable version of [`All::get_by_name`].
    fn get_mut_by_name(&mut self, name: impl AsRef<str>) -> Option<&mut <Self as AllInner>::Gruser> {
        self.iter_mut()
            .find(|gruser| gruser.name() == name.as_ref() )
    }

    /// Borrow the [`User`] or [`Group`] with the given ID.
    ///
    /// # Examples
    ///
    /// Basic usage:
    ///
    /// ```no_run
    /// # use redox_users::{All, AllUsers, Config};
    /// let users = AllUsers::basic(Config::default()).unwrap();
    /// let user = users.get_by_id(0).unwrap();
    /// ```
    fn get_by_id(&self, id: usize) -> Option<&<Self as AllInner>::Gruser> {
        self.iter()
            .find(|gruser| gruser.id() == id )
    }

    /// Mutable version of [`All::get_by_id`].
    fn get_mut_by_id(&mut self, id: usize) -> Option<&mut <Self as AllInner>::Gruser> {
        self.iter_mut()
            .find(|gruser| gruser.id() == id )
    }

    /// Provides an unused id based on the min and max values in the [`Config`]
    /// passed to the `All`'s constructor.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use redox_users::{All, AllUsers, Config};
    /// let users = AllUsers::basic(Config::default()).unwrap();
    /// let uid = users.get_unique_id().expect("no available uid");
    /// ```
    fn get_unique_id(&self) -> Option<usize> {
        for id in self.config().min_id..self.config().max_id {
            if !self.iter().any(|gruser| gruser.id() == id ) {
                return Some(id)
            }
        }
        None
    }

    /// Remove a [`User`] or [`Group`] from this `All` given it's name. If the
    /// Gruser was removed return `true`, else return `false`. This ensures
    /// that the Gruser no longer exists.
    fn remove_by_name(&mut self, name: impl AsRef<str>) -> bool {
        let list = self.list_mut();
        let indx = list.iter()
            .enumerate()
            .find_map(|(indx, gruser)| if gruser.name() == name.as_ref() {
                    Some(indx)
                } else {
                    None
                });
        if let Some(indx) = indx {
            list.remove(indx);
            true
        } else {
            false
        }
    }

    /// Id version of [`All::remove_by_name`].
    fn remove_by_id(&mut self, id: usize) -> bool {
        let list = self.list_mut();
        let indx = list.iter()
            .enumerate()
            .find_map(|(indx, gruser)| if gruser.id() == id {
                    Some(indx)
                } else {
                    None
                });
        if let Some(indx) = indx {
            list.remove(indx);
            true
        } else {
            false
        }
    }
}

/// `AllUsers` provides (borrowed) access to all the users on the system.
/// Note that this struct implements [`All`] for all of its access functions.
///
/// # Notes
/// Note that everything in this section also applies to [`AllGroups`].
///
/// * If you mutate anything owned by an `AllUsers`, you must call the
///   [`AllUsers::save`] in order for those changes to be applied to the system.
/// * The API here is kept small. Most mutating actions can be accomplished via
///   the [`All::get_mut_by_id`] and [`All::get_mut_by_name`]
///   functions.
#[derive(Debug)]
pub struct AllUsers<A> {
    users: Vec<User<A>>,
    config: Config,
    
    // Hold on to the locked fds to prevent race conditions
    passwd_fd: File,
    shadow_fd: Option<File>,
}

impl<A> AllUsers<A> {
    fn new(config: Config) -> Result<AllUsers<A>> {
        let mut passwd_fd = locked_file(config.in_scheme(PASSWD_FILE), Lock::Exclusive)?;
        let mut passwd_cntnt = String::new();
        passwd_fd.read_to_string(&mut passwd_cntnt)?;

        let mut passwd_entries = Vec::new();
        for (indx, line) in passwd_cntnt.lines().enumerate() {
            let mut user = User::from_passwd_entry(line, indx)?;
            user.auth_delay = config.auth_delay;
            passwd_entries.push(user);
        }
        
        Ok(AllUsers::<A> {
            users: passwd_entries,
            config,
            passwd_fd,
            shadow_fd: None,
        })
    }
}

impl AllUsers<auth::Basic> {
    /// Provide access to all user information on the system except
    /// authentication. This is adequate for almost all uses of `AllUsers`.
    pub fn basic(config: Config) -> Result<AllUsers<auth::Basic>> {
        Self::new(config)
    }
}

#[cfg(feature = "auth")]
impl AllUsers<auth::Full> {
    /// If access to password related methods for the [`User`]s yielded by this
    /// `AllUsers` is required, use this constructor.
    pub fn authenticator(config: Config) -> Result<AllUsers<auth::Full>> {
        let mut shadow_fd = locked_file(config.in_scheme(SHADOW_FILE), Lock::Exclusive)?;
        let mut shadow_cntnt = String::new();
        shadow_fd.read_to_string(&mut shadow_cntnt)?;
        let shadow_entries: Vec<&str> = shadow_cntnt.lines().collect();
        
        let mut new = Self::new(config)?;
        new.shadow_fd = Some(shadow_fd);
        
        for (indx, entry) in shadow_entries.iter().enumerate() {
            let mut entry = entry.split(';');
            let name = entry.next().ok_or(parse_error(indx,
                "error parsing shadowfile: expected username"
            ))?;
            let hash = entry.next().ok_or(parse_error(indx,
                "error parsing shadowfile: expected hash"
            ))?;
            new.users
                .iter_mut()
                .find(|user| user.user == name)
                .ok_or(parse_error(indx,
                    "error parsing shadowfile: unkown user"
                ))?
                .populate_hash(hash)?;
        }
        
        Ok(new)
    }
    
    /// Adds a user with the specified attributes to the `AllUsers`
    /// instance. Note that the user's password is set unset (see
    /// [Unset vs Blank Passwords](struct.User.html#unset-vs-blank-passwords))
    /// during this call.
    ///
    /// Make sure to call [`AllUsers::save`] in order for the new user to be
    /// applied to the system.
    //TODO: Take uid/gid as Option<usize> and if none, find an unused ID.
    pub fn add_user(
        &mut self,
        login: &str,
        uid: usize,
        gid: usize,
        name: &str,
        home: &str,
        shell: &str
    ) -> Result<()> {
        if self.iter()
            .any(|user| user.user == login || user.uid == uid)
        {
            return Err(From::from(UsersError::AlreadyExists))
        }

        self.users.push(User {
            user: login.into(),
            uid,
            gid,
            name: name.into(),
            home: home.into(),
            shell: shell.into(),
            hash: Some(("!".into(), false)),
            auth: PhantomData,
            auth_delay: self.config.auth_delay
        });
        Ok(())
    }

    /// Syncs the data stored in the `AllUsers` instance to the filesystem.
    /// To apply changes to the system from an `AllUsers`, you MUST call this
    /// function!
    pub fn save(&mut self) -> Result<()> {
        let mut userstring = String::new();
        let mut shadowstring = String::new();
        for user in &self.users {
            userstring.push_str(&user.passwd_entry());
            shadowstring.push_str(&user.shadow_entry());
        }

        let mut shadow_fd = self.shadow_fd.as_mut()
            .expect("shadow_fd should exist for AllUsers<auth::Full>");

        reset_file(&mut self.passwd_fd)?;
        self.passwd_fd.write_all(userstring.as_bytes())?;

        reset_file(&mut shadow_fd)?;
        shadow_fd.write_all(shadowstring.as_bytes())?;
        Ok(())
    }
}

impl<A> AllInner for AllUsers<A> {
    type Gruser = User<A>;

    fn list(&self) -> &Vec<Self::Gruser> {
        &self.users
    }

    fn list_mut(&mut self) -> &mut Vec<Self::Gruser> {
        &mut self.users
    }

    fn config(&self) -> &Config {
        &self.config
    }
}

impl<A> All for AllUsers<A> {}
/*
#[cfg(not(target_os = "redox"))]
impl<A> Drop for AllUsers<A> {
    fn drop(&mut self) {
        eprintln!("Dropping AllUsers");
        let _ = flock(self.passwd_fd.as_raw_fd(), FlockArg::Unlock);
        if let Some(fd) = self.shadow_fd.as_ref() {
            eprintln!("Shadow");
            let _ = flock(fd.as_raw_fd(), FlockArg::Unlock);
        }
    }
}
*/
/// `AllGroups` provides (borrowed) access to all groups on the system. Note
/// that this struct implements [`All`] for all of its access functions.
///
/// General notes that also apply to this struct may be found with
/// [`AllUsers`].
#[derive(Debug)]
pub struct AllGroups {
    groups: Vec<Group>,
    config: Config,
    
    group_fd: File,
}

impl AllGroups {
    /// Create a new `AllGroups`.
    pub fn new(config: Config) -> Result<AllGroups> {
        let mut group_fd = locked_file(config.in_scheme(GROUP_FILE), Lock::Exclusive)?;
        let mut group_cntnt = String::new();
        group_fd.read_to_string(&mut group_cntnt)?;

        let mut entries: Vec<Group> = Vec::new();
        for (indx, line) in group_cntnt.lines().enumerate() {
            let group = Group::from_group_entry(line, indx)?;
            entries.push(group);
        }

        Ok(AllGroups {
            groups: entries,
            config,
            group_fd,
        })
    }

    /// Adds a group with the specified attributes to this `AllGroups`.
    ///
    /// Make sure to call [`AllGroups::save`] in order for the new group to be
    /// applied to the system.
    //TODO: Take Option<usize> for gid and find unused ID if None
    pub fn add_group(
        &mut self,
        name: &str,
        gid: usize,
        users: &[&str]
    ) -> Result<()> {
        if self.iter()
            .any(|group| group.group == name || group.gid == gid)
        {
            return Err(From::from(UsersError::AlreadyExists))
        }

        //Might be cleaner... Also breaks...
        //users: users.iter().map(String::to_string).collect()
        self.groups.push(Group {
            group: name.into(),
            gid,
            users: users
                .iter()
                .map(|user| user.to_string())
                .collect()
        });

        Ok(())
    }

    /// Syncs the data stored in this `AllGroups` instance to the filesystem.
    /// To apply changes from an `AllGroups`, you MUST call this function!
    pub fn save(&mut self) -> Result<()> {
        let mut groupstring = String::new();
        for group in &self.groups {
            groupstring.push_str(&group.group_entry());
        }

        reset_file(&mut self.group_fd)?;
        self.group_fd.write_all(groupstring.as_bytes())?;
        Ok(())
    }
}

impl AllInner for AllGroups {
    type Gruser = Group;

    fn list(&self) -> &Vec<Self::Gruser> {
        &self.groups
    }

    fn list_mut(&mut self) -> &mut Vec<Self::Gruser> {
        &mut self.groups
    }

    fn config(&self) -> &Config {
        &self.config
    }
}

impl All for AllGroups {}
/*
#[cfg(not(target_os = "redox"))]
impl Drop for AllGroups {
    fn drop(&mut self) {
        eprintln!("Dropping AllGroups");
        let _ = flock(self.group_fd.as_raw_fd(), FlockArg::Unlock);
    }
}*/

#[cfg(test)]
mod test {
    use super::*;

    const TEST_PREFIX: &'static str = "tests";

    /// Needed for the file checks, this is done by the library
    fn test_prefix(filename: &str) -> String {
        let mut complete = String::from(TEST_PREFIX);
        complete.push_str(filename);
        complete
    }

    fn test_cfg() -> Config {
        Config::default()
            // Since all this really does is prepend `sheme` to the consts
            .scheme(TEST_PREFIX.to_string())
    }

    fn read_locked_file(file: impl AsRef<Path>) -> Result<String> {
        let mut fd = locked_file(file, Lock::Exclusive)?;
        let mut cntnt = String::new();
        fd.read_to_string(&mut cntnt)?;
        Ok(cntnt)
    }

    fn write_locked_file(file: impl AsRef<Path>, cntnt: impl AsRef<[u8]>) -> Result<()> {
        locked_file(file, Lock::Exclusive)?
            .write_all(cntnt.as_ref())?;
        Ok(())
    }

    // *** struct.User ***
    #[cfg(feature = "auth")]
    #[test]
    fn attempt_user_api() {
        let mut users = AllUsers::authenticator(test_cfg()).unwrap();
        let user = users.get_mut_by_id(1000).unwrap();

        assert_eq!(user.is_passwd_blank(), true);
        assert_eq!(user.is_passwd_unset(), false);
        assert_eq!(user.verify_passwd(""), true);
        assert_eq!(user.verify_passwd("Something"), false);

        user.set_passwd("hi,i_am_passwd").unwrap();

        assert_eq!(user.is_passwd_blank(), false);
        assert_eq!(user.is_passwd_unset(), false);
        assert_eq!(user.verify_passwd(""), false);
        assert_eq!(user.verify_passwd("Something"), false);
        assert_eq!(user.verify_passwd("hi,i_am_passwd"), true);

        user.unset_passwd();

        assert_eq!(user.is_passwd_blank(), false);
        assert_eq!(user.is_passwd_unset(), true);
        assert_eq!(user.verify_passwd(""), false);
        assert_eq!(user.verify_passwd("Something"), false);
        assert_eq!(user.verify_passwd("hi,i_am_passwd"), false);

        user.set_passwd("").unwrap();

        assert_eq!(user.is_passwd_blank(), true);
        assert_eq!(user.is_passwd_unset(), false);
        assert_eq!(user.verify_passwd(""), true);
        assert_eq!(user.verify_passwd("Something"), false);
    }

    // *** struct.AllUsers ***
    #[cfg(feature = "auth")]
    #[test]
    fn get_user() {
        let users = AllUsers::authenticator(test_cfg()).unwrap();

        let root = users.get_by_id(0).expect("'root' user missing");
        assert_eq!(root.user, "root".to_string());
        let &(ref hashstring, ref encoded) = root.hash.as_ref().expect("'root' hash is None");
        assert_eq!(hashstring,
            &"$argon2i$m=4096,t=10,p=1$Tnc4UVV0N00$ML9LIOujd3nmAfkAwEcSTMPqakWUF0OUiLWrIy0nGLk".to_string());
        assert_eq!(root.uid, 0);
        assert_eq!(root.gid, 0);
        assert_eq!(root.name, "root".to_string());
        assert_eq!(root.home, "file:/root".to_string());
        assert_eq!(root.shell, "file:/bin/ion".to_string());
        match encoded {
            true => (),
            false => panic!("Expected encoded argon hash!")
        }

        let user = users.get_by_name("user").expect("'user' user missing");
        assert_eq!(user.user, "user".to_string());
        let &(ref hashstring, ref encoded) = user.hash.as_ref().expect("'user' hash is None");
        assert_eq!(hashstring, &"".to_string());
        assert_eq!(user.uid, 1000);
        assert_eq!(user.gid, 1000);
        assert_eq!(user.name, "user".to_string());
        assert_eq!(user.home, "file:/home/user".to_string());
        assert_eq!(user.shell, "file:/bin/ion".to_string());
        match encoded {
            true => panic!("Should not be an argon hash!"),
            false => ()
        }
        println!("{:?}", users);

        let li = users.get_by_name("li").expect("'li' user missing");
        println!("got li");
        assert_eq!(li.user, "li");
        let &(ref hashstring, ref encoded) = li.hash.as_ref().expect("'li' hash is None");
        assert_eq!(hashstring, &"!".to_string());
        assert_eq!(li.uid, 1007);
        assert_eq!(li.gid, 1007);
        assert_eq!(li.name, "Lorem".to_string());
        assert_eq!(li.home, "file:/home/lorem".to_string());
        assert_eq!(li.shell, "file:/bin/ion".to_string());
        match encoded {
            true => panic!("Should not be an argon hash!"),
            false => ()
        }
    }

    #[cfg(feature = "auth")]
    #[test]
    fn manip_user() {
        let mut users = AllUsers::authenticator(test_cfg()).unwrap();
        // NOT testing `get_unique_id`
        let id = 7099;
        users
            .add_user("fb", id, id, "Foo Bar", "/home/foob", "/bin/zsh")
            .expect("failed to add user 'fb'");
        //                                            weirdo ^^^^^^^^ :P
        users.save().unwrap();
        let p_file_content = read_locked_file(test_prefix(PASSWD_FILE)).unwrap();
        assert_eq!(
            p_file_content,
            concat!(
                "root;0;0;root;file:/root;file:/bin/ion\n",
                "user;1000;1000;user;file:/home/user;file:/bin/ion\n",
                "li;1007;1007;Lorem;file:/home/lorem;file:/bin/ion\n",
                "fb;7099;7099;Foo Bar;/home/foob;/bin/zsh\n"
            )
        );
        let s_file_content = read_locked_file(test_prefix(SHADOW_FILE)).unwrap();
        assert_eq!(s_file_content, concat!(
            "root;$argon2i$m=4096,t=10,p=1$Tnc4UVV0N00$ML9LIOujd3nmAfkAwEcSTMPqakWUF0OUiLWrIy0nGLk\n",
            "user;\n",
            "li;!\n",
            "fb;!\n"
        ));

        {
            println!("{:?}", users);
            let fb = users.get_mut_by_name("fb").expect("'fb' user missing");
            fb.shell = "/bin/fish".to_string(); // That's better
            fb.set_passwd("").unwrap();
        }
        users.save().unwrap();
        let p_file_content = read_locked_file(test_prefix(PASSWD_FILE)).unwrap();
        assert_eq!(
            p_file_content,
            concat!(
                "root;0;0;root;file:/root;file:/bin/ion\n",
                "user;1000;1000;user;file:/home/user;file:/bin/ion\n",
                "li;1007;1007;Lorem;file:/home/lorem;file:/bin/ion\n",
                "fb;7099;7099;Foo Bar;/home/foob;/bin/fish\n"
            )
        );
        let s_file_content = read_locked_file(test_prefix(SHADOW_FILE)).unwrap();
        assert_eq!(s_file_content, concat!(
            "root;$argon2i$m=4096,t=10,p=1$Tnc4UVV0N00$ML9LIOujd3nmAfkAwEcSTMPqakWUF0OUiLWrIy0nGLk\n",
            "user;\n",
            "li;!\n",
            "fb;\n"
        ));

        users.remove_by_id(id);
        users.save().unwrap();
        let file_content = read_locked_file(test_prefix(PASSWD_FILE)).unwrap();
        assert_eq!(
            file_content,
            concat!(
                "root;0;0;root;file:/root;file:/bin/ion\n",
                "user;1000;1000;user;file:/home/user;file:/bin/ion\n",
                "li;1007;1007;Lorem;file:/home/lorem;file:/bin/ion\n"
            )
        );
    }

    /* struct.Group */
    #[test]
    fn empty_groups() {
        let group_trailing = Group::from_group_entry("nobody;2066; ", 0).unwrap();
        assert_eq!(group_trailing.users.len(), 0);
        
        let group_no_trailing = Group::from_group_entry("nobody;2066;", 0).unwrap();
        assert_eq!(group_no_trailing.users.len(), 0);
        
        assert_eq!(group_trailing.group, group_no_trailing.group);
        assert_eq!(group_trailing.gid, group_no_trailing.gid);
        assert_eq!(group_trailing.users, group_no_trailing.users);
    }

    /* struct.AllGroups */
    #[test]
    fn get_group() {
        let groups = AllGroups::new(test_cfg()).unwrap();
        let user = groups.get_by_name("user").unwrap();
        assert_eq!(user.group, "user");
        assert_eq!(user.gid, 1000);
        assert_eq!(user.users, vec!["user"]);

        let wheel = groups.get_by_id(1).unwrap();
        assert_eq!(wheel.group, "wheel");
        assert_eq!(wheel.gid, 1);
        assert_eq!(wheel.users, vec!["user", "root"]);
    }

    #[test]
    fn manip_group() {
        let mut groups = AllGroups::new(test_cfg()).unwrap();
        // NOT testing `get_unique_id`
        let id = 7099;

        groups.add_group("fb", id, &["fb"]).unwrap();
        groups.save().unwrap();
        let file_content = read_locked_file(test_prefix(GROUP_FILE)).unwrap();
        assert_eq!(
            file_content,
            concat!(
                "root;0;root\n",
                "user;1000;user\n",
                "wheel;1;user,root\n",
                "li;1007;li\n",
                "fb;7099;fb\n"
            )
        );

        {
            let fb = groups.get_mut_by_name("fb").unwrap();
            fb.users.push("user".to_string());
        }
        groups.save().unwrap();
        let file_content = read_locked_file(test_prefix(GROUP_FILE)).unwrap();
        assert_eq!(
            file_content,
            concat!(
                "root;0;root\n",
                "user;1000;user\n",
                "wheel;1;user,root\n",
                "li;1007;li\n",
                "fb;7099;fb,user\n"
            )
        );

        groups.remove_by_id(id);
        groups.save().unwrap();
        let file_content = read_locked_file(test_prefix(GROUP_FILE)).unwrap();
        assert_eq!(
            file_content,
            concat!(
                "root;0;root\n",
                "user;1000;user\n",
                "wheel;1;user,root\n",
                "li;1007;li\n"
            )
        );
    }
    
    #[test]
    fn empty_group() {
        let mut groups = AllGroups::new(test_cfg()).unwrap();
        
        groups.add_group("nobody", 2260, &[]).unwrap();
        groups.save().unwrap();
        let file_content = read_locked_file(test_prefix(GROUP_FILE)).unwrap();
        assert_eq!(
            file_content,
            concat!(
                "root;0;root\n",
                "user;1000;user\n",
                "wheel;1;user,root\n",
                "li;1007;li\n",
                "nobody;2260;\n",
            )
        );
        
        drop(groups);
        let mut groups = AllGroups::new(test_cfg()).unwrap();
        
        groups.remove_by_name("nobody");
        groups.save().unwrap();
        
        let file_content = read_locked_file(test_prefix(GROUP_FILE)).unwrap();
        assert_eq!(
            file_content,
            concat!(
                "root;0;root\n",
                "user;1000;user\n",
                "wheel;1;user,root\n",
                "li;1007;li\n"
            )
        );
    }

    // *** Misc ***
    #[test]
    fn users_get_unused_ids() {
        let users = AllUsers::basic(test_cfg()).unwrap();
        let id = users.get_unique_id().unwrap();
        if id < users.config.min_id || id > users.config.max_id {
            panic!("User ID is not between allowed margins")
        } else if let Some(_) = users.get_by_id(id) {
            panic!("User ID is used!");
        }
    }

    #[test]
    fn groups_get_unused_ids() {
        let groups = AllGroups::new(test_cfg()).unwrap();
        let id = groups.get_unique_id().unwrap();
        if id < groups.config.min_id || id > groups.config.max_id {
            panic!("Group ID is not between allowed margins")
        } else if let Some(_) = groups.get_by_id(id) {
            panic!("Group ID is used!");
        }
    }
}
