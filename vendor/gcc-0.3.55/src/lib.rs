//! A library for build scripts to compile custom C code
//!
//! This library is intended to be used as a `build-dependencies` entry in
//! `Cargo.toml`:
//!
//! ```toml
//! [build-dependencies]
//! gcc = "0.3"
//! ```
//!
//! The purpose of this crate is to provide the utility functions necessary to
//! compile C code into a static archive which is then linked into a Rust crate.
//! Configuration is available through the `Build` struct.
//!
//! This crate will automatically detect situations such as cross compilation or
//! other environment variables set by Cargo and will build code appropriately.
//!
//! The crate is not limited to C code, it can accept any source code that can
//! be passed to a C or C++ compiler. As such, assembly files with extensions
//! `.s` (gcc/clang) and `.asm` (MSVC) can also be compiled.
//!
//! [`Build`]: struct.Build.html
//!
//! # Examples
//!
//! Use the `Build` struct to compile `src/foo.c`:
//!
//! ```no_run
//! extern crate gcc;
//!
//! fn main() {
//!     gcc::Build::new()
//!                .file("src/foo.c")
//!                .define("FOO", Some("bar"))
//!                .include("src")
//!                .compile("foo");
//! }
//! ```

#![doc(html_root_url = "https://docs.rs/gcc/0.3")]
#![cfg_attr(test, deny(warnings))]
#![deny(missing_docs)]

#[cfg(feature = "parallel")]
extern crate rayon;

use std::env;
use std::ffi::{OsString, OsStr};
use std::fs;
use std::path::{PathBuf, Path};
use std::process::{Command, Stdio, Child};
use std::io::{self, BufReader, BufRead, Read, Write};
use std::thread::{self, JoinHandle};

#[doc(hidden)]
#[deprecated(since="0.3.51", note="gcc::Config has been renamed to gcc::Build")]
pub type Config = Build;

#[cfg(feature = "parallel")]
use std::sync::Mutex;

// These modules are all glue to support reading the MSVC version from
// the registry and from COM interfaces
#[cfg(windows)]
mod registry;
#[cfg(windows)]
#[macro_use]
mod winapi;
#[cfg(windows)]
mod com;
#[cfg(windows)]
mod setup_config;

pub mod windows_registry;

/// Extra configuration to pass to gcc.
#[derive(Clone, Debug)]
#[deprecated(note = "crate has been renamed to `cc`, the `gcc` name is not maintained")]
pub struct Build {
    include_directories: Vec<PathBuf>,
    definitions: Vec<(String, Option<String>)>,
    objects: Vec<PathBuf>,
    flags: Vec<String>,
    flags_supported: Vec<String>,
    files: Vec<PathBuf>,
    cpp: bool,
    cpp_link_stdlib: Option<Option<String>>,
    cpp_set_stdlib: Option<String>,
    target: Option<String>,
    host: Option<String>,
    out_dir: Option<PathBuf>,
    opt_level: Option<String>,
    debug: Option<bool>,
    env: Vec<(OsString, OsString)>,
    compiler: Option<PathBuf>,
    archiver: Option<PathBuf>,
    cargo_metadata: bool,
    pic: Option<bool>,
    static_crt: Option<bool>,
    shared_flag: Option<bool>,
    static_flag: Option<bool>,
    warnings_into_errors: bool,
    warnings: bool,
}

/// Represents the types of errors that may occur while using gcc-rs.
#[derive(Clone, Debug)]
enum ErrorKind {
    /// Error occurred while performing I/O.
    IOError,
    /// Invalid architecture supplied.
    ArchitectureInvalid,
    /// Environment variable not found, with the var in question as extra info.
    EnvVarNotFound,
    /// Error occurred while using external tools (ie: invocation of compiler).
    ToolExecError,
    /// Error occurred due to missing external tools.
    ToolNotFound,
}

/// Represents an internal error that occurred, with an explaination.
#[derive(Clone, Debug)]
pub struct Error {
    /// Describes the kind of error that occurred.
    kind: ErrorKind,
    /// More explaination of error that occurred.
    message: String,
}

impl Error {
    fn new(kind: ErrorKind, message: &str) -> Error {
        Error { kind: kind, message: message.to_owned() }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::new(ErrorKind::IOError, &format!("{}", e))
    }
}

/// Configuration used to represent an invocation of a C compiler.
///
/// This can be used to figure out what compiler is in use, what the arguments
/// to it are, and what the environment variables look like for the compiler.
/// This can be used to further configure other build systems (e.g. forward
/// along CC and/or CFLAGS) or the `to_command` method can be used to run the
/// compiler itself.
#[derive(Clone, Debug)]
pub struct Tool {
    path: PathBuf,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    family: ToolFamily
}

/// Represents the family of tools this tool belongs to.
///
/// Each family of tools differs in how and what arguments they accept.
///
/// Detection of a family is done on best-effort basis and may not accurately reflect the tool.
#[derive(Copy, Clone, Debug, PartialEq)]
enum ToolFamily {
    /// Tool is GNU Compiler Collection-like.
    Gnu,
    /// Tool is Clang-like. It differs from the GCC in a sense that it accepts superset of flags
    /// and its cross-compilation approach is different.
    Clang,
    /// Tool is the MSVC cl.exe.
    Msvc,
}

impl ToolFamily {
    /// What the flag to request debug info for this family of tools look like
    fn debug_flag(&self) -> &'static str {
        match *self {
            ToolFamily::Msvc => "/Z7",
            ToolFamily::Gnu |
            ToolFamily::Clang => "-g",
        }
    }

    /// What the flag to include directories into header search path looks like
    fn include_flag(&self) -> &'static str {
        match *self {
            ToolFamily::Msvc => "/I",
            ToolFamily::Gnu |
            ToolFamily::Clang => "-I",
        }
    }

    /// What the flag to request macro-expanded source output looks like
    fn expand_flag(&self) -> &'static str {
        match *self {
            ToolFamily::Msvc => "/E",
            ToolFamily::Gnu |
            ToolFamily::Clang => "-E",
        }
    }

    /// What the flags to enable all warnings
    fn warnings_flags(&self) -> &'static [&'static str] {
        static MSVC_FLAGS: &'static [&'static str] = &["/W4"];
        static GNU_CLANG_FLAGS: &'static [&'static str] = &["-Wall", "-Wextra"];

        match *self {
            ToolFamily::Msvc => &MSVC_FLAGS,
            ToolFamily::Gnu |
            ToolFamily::Clang => &GNU_CLANG_FLAGS,
        }
    }

    /// What the flag to turn warning into errors
    fn warnings_to_errors_flag(&self) -> &'static str {
        match *self {
            ToolFamily::Msvc => "/WX",
            ToolFamily::Gnu |
            ToolFamily::Clang => "-Werror"
        }
    }
}

/// Compile a library from the given set of input C files.
///
/// This will simply compile all files into object files and then assemble them
/// into the output. This will read the standard environment variables to detect
/// cross compilations and such.
///
/// This function will also print all metadata on standard output for Cargo.
///
/// # Example
///
/// ```no_run
/// gcc::compile_library("foo", &["foo.c", "bar.c"]);
/// ```
#[doc(hidden)]
#[deprecated(note = "crate has been renamed to `cc`, the `gcc` name is not maintained")]
pub fn compile_library(output: &str, files: &[&str]) {
    let mut c = Build::new();
    for f in files.iter() {
        c.file(*f);
    }
    c.compile(output);
}

impl Build {
    /// Construct a new instance of a blank set of configuration.
    ///
    /// This builder is finished with the [`compile`] function.
    ///
    /// [`compile`]: struct.Build.html#method.compile
    #[deprecated(note = "crate has been renamed to `cc`, the `gcc` name is not maintained")]
    pub fn new() -> Build {
        Build {
            include_directories: Vec::new(),
            definitions: Vec::new(),
            objects: Vec::new(),
            flags: Vec::new(),
            flags_supported: Vec::new(),
            files: Vec::new(),
            shared_flag: None,
            static_flag: None,
            cpp: false,
            cpp_link_stdlib: None,
            cpp_set_stdlib: None,
            target: None,
            host: None,
            out_dir: None,
            opt_level: None,
            debug: None,
            env: Vec::new(),
            compiler: None,
            archiver: None,
            cargo_metadata: true,
            pic: None,
            static_crt: None,
            warnings: true,
            warnings_into_errors: false,
        }
    }

    /// Add a directory to the `-I` or include path for headers
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::path::Path;
    ///
    /// let library_path = Path::new("/path/to/library");
    ///
    /// gcc::Build::new()
    ///            .file("src/foo.c")
    ///            .include(library_path)
    ///            .include("src")
    ///            .compile("foo");
    /// ```
    pub fn include<P: AsRef<Path>>(&mut self, dir: P) -> &mut Build {
        self.include_directories.push(dir.as_ref().to_path_buf());
        self
    }

    /// Specify a `-D` variable with an optional value.
    ///
    /// # Example
    ///
    /// ```no_run
    /// gcc::Build::new()
    ///            .file("src/foo.c")
    ///            .define("FOO", "BAR")
    ///            .define("BAZ", None)
    ///            .compile("foo");
    /// ```
    pub fn define<'a, V: Into<Option<&'a str>>>(&mut self, var: &str, val: V) -> &mut Build {
        self.definitions.push((var.to_string(), val.into().map(|s| s.to_string())));
        self
    }

    /// Add an arbitrary object file to link in
    pub fn object<P: AsRef<Path>>(&mut self, obj: P) -> &mut Build {
        self.objects.push(obj.as_ref().to_path_buf());
        self
    }

    /// Add an arbitrary flag to the invocation of the compiler
    ///
    /// # Example
    ///
    /// ```no_run
    /// gcc::Build::new()
    ///            .file("src/foo.c")
    ///            .flag("-ffunction-sections")
    ///            .compile("foo");
    /// ```
    pub fn flag(&mut self, flag: &str) -> &mut Build {
        self.flags.push(flag.to_string());
        self
    }

    fn ensure_check_file(&self) -> Result<PathBuf, Error> {
        let out_dir = self.get_out_dir()?;
        let src = if self.cpp {
            out_dir.join("flag_check.cpp")
        } else {
            out_dir.join("flag_check.c")
        };

        if !src.exists() {
            let mut f = fs::File::create(&src)?;
            write!(f, "int main(void) {{ return 0; }}")?;
        }

        Ok(src)
    }

    fn is_flag_supported(&self, flag: &str) -> Result<bool, Error> {
        let out_dir = self.get_out_dir()?;
        let src = self.ensure_check_file()?;
        let obj = out_dir.join("flag_check");
        let target = self.get_target()?;
        let mut cfg = Build::new();
        cfg.flag(flag)
           .target(&target)
           .opt_level(0)
           .host(&target)
           .debug(false)
           .cpp(self.cpp);
        let compiler = cfg.try_get_compiler()?;
        let mut cmd = compiler.to_command();
        command_add_output_file(&mut cmd, &obj, target.contains("msvc"), false);
        cmd.arg(&src);

        let output = cmd.output()?;
        Ok(output.stderr.is_empty())
    }

    /// Add an arbitrary flag to the invocation of the compiler if it supports it
    ///
    /// # Example
    ///
    /// ```no_run
    /// gcc::Build::new()
    ///            .file("src/foo.c")
    ///            .flag_if_supported("-Wlogical-op") // only supported by GCC
    ///            .flag_if_supported("-Wunreachable-code") // only supported by clang
    ///            .compile("foo");
    /// ```
    pub fn flag_if_supported(&mut self, flag: &str) -> &mut Build {
        self.flags_supported.push(flag.to_string());
        self
    }

    /// Set the `-shared` flag.
    ///
    /// When enabled, the compiler will produce a shared object which can
    /// then be linked with other objects to form an executable.
    ///
    /// # Example
    ///
    /// ```no_run
    /// gcc::Build::new()
    ///            .file("src/foo.c")
    ///            .shared_flag(true)
    ///            .compile("libfoo.so");
    /// ```

    pub fn shared_flag(&mut self, shared_flag: bool) -> &mut Build {
        self.shared_flag = Some(shared_flag);
        self
    }

    /// Set the `-static` flag.
    ///
    /// When enabled on systems that support dynamic linking, this prevents
    /// linking with the shared libraries.
    ///
    /// # Example
    ///
    /// ```no_run
    /// gcc::Build::new()
    ///            .file("src/foo.c")
    ///            .shared_flag(true)
    ///            .static_flag(true)
    ///            .compile("foo");
    /// ```
    pub fn static_flag(&mut self, static_flag: bool) -> &mut Build {
        self.static_flag = Some(static_flag);
        self
    }

    /// Add a file which will be compiled
    pub fn file<P: AsRef<Path>>(&mut self, p: P) -> &mut Build {
        self.files.push(p.as_ref().to_path_buf());
        self
    }

    /// Add files which will be compiled
    pub fn files<P>(&mut self, p: P) -> &mut Build
        where P: IntoIterator,
              P::Item: AsRef<Path> {
        for file in p.into_iter() {
            self.file(file);
        }
        self
    }

    /// Set C++ support.
    ///
    /// The other `cpp_*` options will only become active if this is set to
    /// `true`.
    pub fn cpp(&mut self, cpp: bool) -> &mut Build {
        self.cpp = cpp;
        self
    }

    /// Set warnings into errors flag.
    ///
    /// Disabled by default.
    ///
    /// Warning: turning warnings into errors only make sense
    /// if you are a developer of the crate using gcc-rs.
    /// Some warnings only appear on some architecture or
    /// specific version of the compiler. Any user of this crate,
    /// or any other crate depending on it, could fail during
    /// compile time.
    ///
    /// # Example
    ///
    /// ```no_run
    /// gcc::Build::new()
    ///            .file("src/foo.c")
    ///            .warnings_into_errors(true)
    ///            .compile("libfoo.a");
    /// ```
    pub fn warnings_into_errors(&mut self, warnings_into_errors: bool) -> &mut Build {
        self.warnings_into_errors = warnings_into_errors;
        self
    }

    /// Set warnings flags.
    ///
    /// Adds some flags:
    /// - "/Wall" for MSVC.
    /// - "-Wall", "-Wextra" for GNU and Clang.
    ///
    /// Enabled by default.
    ///
    /// # Example
    ///
    /// ```no_run
    /// gcc::Build::new()
    ///            .file("src/foo.c")
    ///            .warnings(false)
    ///            .compile("libfoo.a");
    /// ```
    pub fn warnings(&mut self, warnings: bool) -> &mut Build {
        self.warnings = warnings;
        self
    }

    /// Set the standard library to link against when compiling with C++
    /// support.
    ///
    /// The default value of this property depends on the current target: On
    /// OS X `Some("c++")` is used, when compiling for a Visual Studio based
    /// target `None` is used and for other targets `Some("stdc++")` is used.
    ///
    /// A value of `None` indicates that no automatic linking should happen,
    /// otherwise cargo will link against the specified library.
    ///
    /// The given library name must not contain the `lib` prefix.
    ///
    /// Common values:
    /// - `stdc++` for GNU
    /// - `c++` for Clang
    ///
    /// # Example
    ///
    /// ```no_run
    /// gcc::Build::new()
    ///            .file("src/foo.c")
    ///            .shared_flag(true)
    ///            .cpp_link_stdlib("stdc++")
    ///            .compile("libfoo.so");
    /// ```
    pub fn cpp_link_stdlib<'a, V: Into<Option<&'a str>>>(&mut self, cpp_link_stdlib: V) -> &mut Build {
        self.cpp_link_stdlib = Some(cpp_link_stdlib.into().map(|s| s.into()));
        self
    }

    /// Force the C++ compiler to use the specified standard library.
    ///
    /// Setting this option will automatically set `cpp_link_stdlib` to the same
    /// value.
    ///
    /// The default value of this option is always `None`.
    ///
    /// This option has no effect when compiling for a Visual Studio based
    /// target.
    ///
    /// This option sets the `-stdlib` flag, which is only supported by some
    /// compilers (clang, icc) but not by others (gcc). The library will not
    /// detect which compiler is used, as such it is the responsibility of the
    /// caller to ensure that this option is only used in conjuction with a
    /// compiler which supports the `-stdlib` flag.
    ///
    /// A value of `None` indicates that no specific C++ standard library should
    /// be used, otherwise `-stdlib` is added to the compile invocation.
    ///
    /// The given library name must not contain the `lib` prefix.
    ///
    /// Common values:
    /// - `stdc++` for GNU
    /// - `c++` for Clang
    ///
    /// # Example
    ///
    /// ```no_run
    /// gcc::Build::new()
    ///            .file("src/foo.c")
    ///            .cpp_set_stdlib("c++")
    ///            .compile("libfoo.a");
    /// ```
    pub fn cpp_set_stdlib<'a, V: Into<Option<&'a str>>>(&mut self, cpp_set_stdlib: V) -> &mut Build {
        let cpp_set_stdlib = cpp_set_stdlib.into();
        self.cpp_set_stdlib = cpp_set_stdlib.map(|s| s.into());
        self.cpp_link_stdlib(cpp_set_stdlib);
        self
    }

    /// Configures the target this configuration will be compiling for.
    ///
    /// This option is automatically scraped from the `TARGET` environment
    /// variable by build scripts, so it's not required to call this function.
    ///
    /// # Example
    ///
    /// ```no_run
    /// gcc::Build::new()
    ///            .file("src/foo.c")
    ///            .target("aarch64-linux-android")
    ///            .compile("foo");
    /// ```
    pub fn target(&mut self, target: &str) -> &mut Build {
        self.target = Some(target.to_string());
        self
    }

    /// Configures the host assumed by this configuration.
    ///
    /// This option is automatically scraped from the `HOST` environment
    /// variable by build scripts, so it's not required to call this function.
    ///
    /// # Example
    ///
    /// ```no_run
    /// gcc::Build::new()
    ///            .file("src/foo.c")
    ///            .host("arm-linux-gnueabihf")
    ///            .compile("foo");
    /// ```
    pub fn host(&mut self, host: &str) -> &mut Build {
        self.host = Some(host.to_string());
        self
    }

    /// Configures the optimization level of the generated object files.
    ///
    /// This option is automatically scraped from the `OPT_LEVEL` environment
    /// variable by build scripts, so it's not required to call this function.
    pub fn opt_level(&mut self, opt_level: u32) -> &mut Build {
        self.opt_level = Some(opt_level.to_string());
        self
    }

    /// Configures the optimization level of the generated object files.
    ///
    /// This option is automatically scraped from the `OPT_LEVEL` environment
    /// variable by build scripts, so it's not required to call this function.
    pub fn opt_level_str(&mut self, opt_level: &str) -> &mut Build {
        self.opt_level = Some(opt_level.to_string());
        self
    }

    /// Configures whether the compiler will emit debug information when
    /// generating object files.
    ///
    /// This option is automatically scraped from the `PROFILE` environment
    /// variable by build scripts (only enabled when the profile is "debug"), so
    /// it's not required to call this function.
    pub fn debug(&mut self, debug: bool) -> &mut Build {
        self.debug = Some(debug);
        self
    }

    /// Configures the output directory where all object files and static
    /// libraries will be located.
    ///
    /// This option is automatically scraped from the `OUT_DIR` environment
    /// variable by build scripts, so it's not required to call this function.
    pub fn out_dir<P: AsRef<Path>>(&mut self, out_dir: P) -> &mut Build {
        self.out_dir = Some(out_dir.as_ref().to_owned());
        self
    }

    /// Configures the compiler to be used to produce output.
    ///
    /// This option is automatically determined from the target platform or a
    /// number of environment variables, so it's not required to call this
    /// function.
    pub fn compiler<P: AsRef<Path>>(&mut self, compiler: P) -> &mut Build {
        self.compiler = Some(compiler.as_ref().to_owned());
        self
    }

    /// Configures the tool used to assemble archives.
    ///
    /// This option is automatically determined from the target platform or a
    /// number of environment variables, so it's not required to call this
    /// function.
    pub fn archiver<P: AsRef<Path>>(&mut self, archiver: P) -> &mut Build {
        self.archiver = Some(archiver.as_ref().to_owned());
        self
    }
    /// Define whether metadata should be emitted for cargo allowing it to
    /// automatically link the binary. Defaults to `true`.
    ///
    /// The emitted metadata is:
    ///
    ///  - `rustc-link-lib=static=`*compiled lib*
    ///  - `rustc-link-search=native=`*target folder*
    ///  - When target is MSVC, the ATL-MFC libs are added via `rustc-link-search=native=`
    ///  - When C++ is enabled, the C++ stdlib is added via `rustc-link-lib`
    ///
    pub fn cargo_metadata(&mut self, cargo_metadata: bool) -> &mut Build {
        self.cargo_metadata = cargo_metadata;
        self
    }

    /// Configures whether the compiler will emit position independent code.
    ///
    /// This option defaults to `false` for `windows-gnu` targets and
    /// to `true` for all other targets.
    pub fn pic(&mut self, pic: bool) -> &mut Build {
        self.pic = Some(pic);
        self
    }

    /// Configures whether the /MT flag or the /MD flag will be passed to msvc build tools.
    ///
    /// This option defaults to `false`, and affect only msvc targets.
    pub fn static_crt(&mut self, static_crt: bool) -> &mut Build {
        self.static_crt = Some(static_crt);
        self
    }

    #[doc(hidden)]
    pub fn __set_env<A, B>(&mut self, a: A, b: B) -> &mut Build
        where A: AsRef<OsStr>,
              B: AsRef<OsStr>
    {
        self.env.push((a.as_ref().to_owned(), b.as_ref().to_owned()));
        self
    }

    /// Run the compiler, generating the file `output`
    ///
    /// This will return a result instead of panicing; see compile() for the complete description.
    pub fn try_compile(&self, output: &str) -> Result<(), Error> {
        let (lib_name, gnu_lib_name) = if output.starts_with("lib") && output.ends_with(".a") {
                (&output[3..output.len() - 2], output.to_owned())
            } else {
                let mut gnu = String::with_capacity(5 + output.len());
                gnu.push_str("lib");
                gnu.push_str(&output);
                gnu.push_str(".a");
                (output, gnu)
            };
        let dst = self.get_out_dir()?;

        let mut objects = Vec::new();
        let mut src_dst = Vec::new();
        for file in self.files.iter() {
            let obj = dst.join(file).with_extension("o");
            let obj = if !obj.starts_with(&dst) {
                dst.join(obj.file_name().ok_or_else(|| Error::new(ErrorKind::IOError, "Getting object file details failed."))?)
            } else {
                obj
            };

            match obj.parent() {
                Some(s) => fs::create_dir_all(s)?,
                None => return Err(Error::new(ErrorKind::IOError, "Getting object file details failed.")),
            };

            src_dst.push((file.to_path_buf(), obj.clone()));
            objects.push(obj);
        }
        self.compile_objects(&src_dst)?;
        self.assemble(lib_name, &dst.join(gnu_lib_name), &objects)?;

        if self.get_target()?.contains("msvc") {
            let compiler = self.get_base_compiler()?;
            let atlmfc_lib = compiler.env()
                .iter()
                .find(|&&(ref var, _)| var.as_os_str() == OsStr::new("LIB"))
                .and_then(|&(_, ref lib_paths)| {
                    env::split_paths(lib_paths).find(|path| {
                        let sub = Path::new("atlmfc/lib");
                        path.ends_with(sub) || path.parent().map_or(false, |p| p.ends_with(sub))
                    })
                });

            if let Some(atlmfc_lib) = atlmfc_lib {
                self.print(&format!("cargo:rustc-link-search=native={}", atlmfc_lib.display()));
            }
        }

        self.print(&format!("cargo:rustc-link-lib=static={}", lib_name));
        self.print(&format!("cargo:rustc-link-search=native={}", dst.display()));

        // Add specific C++ libraries, if enabled.
        if self.cpp {
            if let Some(stdlib) = self.get_cpp_link_stdlib()? {
                self.print(&format!("cargo:rustc-link-lib={}", stdlib));
            }
        }

        Ok(())
    }

    /// Run the compiler, generating the file `output`
    ///
    /// The name `output` should be the name of the library.  For backwards compatibility,
    /// the `output` may start with `lib` and end with `.a`.  The Rust compilier will create
    /// the assembly with the lib prefix and .a extension.  MSVC will create a file without prefix,
    /// ending with `.lib`.
    ///
    /// # Panics
    ///
    /// Panics if `output` is not formatted correctly or if one of the underlying
    /// compiler commands fails. It can also panic if it fails reading file names
    /// or creating directories.
    pub fn compile(&self, output: &str) {
        if let Err(e) = self.try_compile(output) {
            fail(&e.message);
        }
    }

    #[cfg(feature = "parallel")]
    fn compile_objects(&self, objs: &[(PathBuf, PathBuf)]) -> Result<(), Error> {
        use self::rayon::prelude::*;

        let mut cfg = rayon::Configuration::new();
        if let Ok(amt) = env::var("NUM_JOBS") {
            if let Ok(amt) = amt.parse() {
                cfg = cfg.num_threads(amt);
            }
        }
        drop(rayon::initialize(cfg));

        let results: Mutex<Vec<Result<(), Error>>> = Mutex::new(Vec::new());

        objs.par_iter().with_max_len(1)
            .for_each(|&(ref src, ref dst)| results.lock().unwrap().push(self.compile_object(src, dst)));

        // Check for any errors and return the first one found.
        for result in results.into_inner().unwrap().iter() {
            if result.is_err() {
                return result.clone();
            }
        }

        Ok(())
    }

    #[cfg(not(feature = "parallel"))]
    fn compile_objects(&self, objs: &[(PathBuf, PathBuf)]) -> Result<(), Error> {
        for &(ref src, ref dst) in objs {
            self.compile_object(src, dst)?;
        }
        Ok(())
    }

    fn compile_object(&self, file: &Path, dst: &Path) -> Result<(), Error> {
        let is_asm = file.extension().and_then(|s| s.to_str()) == Some("asm");
        let msvc = self.get_target()?.contains("msvc");
        let (mut cmd, name) = if msvc && is_asm {
            self.msvc_macro_assembler()?
        } else {
            let compiler = self.try_get_compiler()?;
            let mut cmd = compiler.to_command();
            for &(ref a, ref b) in self.env.iter() {
                cmd.env(a, b);
            }
            (cmd,
             compiler.path
                 .file_name()
                 .ok_or_else(|| Error::new(ErrorKind::IOError, "Failed to get compiler path."))?
                 .to_string_lossy()
                 .into_owned())
        };
        command_add_output_file(&mut cmd, dst, msvc, is_asm);
        cmd.arg(if msvc { "/c" } else { "-c" });
        cmd.arg(file);

        run(&mut cmd, &name)?;
        Ok(())
    }

    /// This will return a result instead of panicing; see expand() for the complete description.
    pub fn try_expand(&self) -> Result<Vec<u8>, Error> {
        let compiler = self.try_get_compiler()?;
        let mut cmd = compiler.to_command();
        for &(ref a, ref b) in self.env.iter() {
            cmd.env(a, b);
        }
        cmd.arg(compiler.family.expand_flag());

        assert!(self.files.len() <= 1,
                "Expand may only be called for a single file");

        for file in self.files.iter() {
            cmd.arg(file);
        }

        let name = compiler.path
            .file_name()
            .ok_or_else(|| Error::new(ErrorKind::IOError, "Failed to get compiler path."))?
            .to_string_lossy()
            .into_owned();

        Ok(run_output(&mut cmd, &name)?)
    }

    /// Run the compiler, returning the macro-expanded version of the input files.
    ///
    /// This is only relevant for C and C++ files.
    ///
    /// # Panics
    /// Panics if more than one file is present in the config, or if compiler
    /// path has an invalid file name.
    ///
    /// # Example
    /// ```no_run
    /// let out = gcc::Build::new()
    ///                       .file("src/foo.c")
    ///                       .expand();
    /// ```
    pub fn expand(&self) -> Vec<u8> {
        match self.try_expand() {
            Err(e) => fail(&e.message),
            Ok(v) => v,
        }
    }

    /// Get the compiler that's in use for this configuration.
    ///
    /// This function will return a `Tool` which represents the culmination
    /// of this configuration at a snapshot in time. The returned compiler can
    /// be inspected (e.g. the path, arguments, environment) to forward along to
    /// other tools, or the `to_command` method can be used to invoke the
    /// compiler itself.
    ///
    /// This method will take into account all configuration such as debug
    /// information, optimization level, include directories, defines, etc.
    /// Additionally, the compiler binary in use follows the standard
    /// conventions for this path, e.g. looking at the explicitly set compiler,
    /// environment variables (a number of which are inspected here), and then
    /// falling back to the default configuration.
    ///
    /// # Panics
    ///
    /// Panics if an error occurred while determining the architecture.
    pub fn get_compiler(&self) -> Tool {
        match self.try_get_compiler() {
            Ok(tool) => tool,
            Err(e) => fail(&e.message),
        }
    }

    /// Get the compiler that's in use for this configuration.
    ///
    /// This will return a result instead of panicing; see get_compiler() for the complete description.
    pub fn try_get_compiler(&self) -> Result<Tool, Error> {
        let opt_level = self.get_opt_level()?;
        let target = self.get_target()?;

        let mut cmd = self.get_base_compiler()?;
        let nvcc = cmd.path.file_name()
            .and_then(|p| p.to_str()).map(|p| p.contains("nvcc"))
            .unwrap_or(false);

        // Non-target flags
        // If the flag is not conditioned on target variable, it belongs here :)
        match cmd.family {
            ToolFamily::Msvc => {
                cmd.args.push("/nologo".into());

                let crt_flag = match self.static_crt {
                    Some(true) => "/MT",
                    Some(false) => "/MD",
                    None => {
                        let features = env::var("CARGO_CFG_TARGET_FEATURE")
                                  .unwrap_or(String::new());
                        if features.contains("crt-static") {
                            "/MT"
                        } else {
                            "/MD"
                        }
                    },
                };
                cmd.args.push(crt_flag.into());

                match &opt_level[..] {
                    "z" | "s" => cmd.args.push("/Os".into()),
                    "1" => cmd.args.push("/O1".into()),
                    // -O3 is a valid value for gcc and clang compilers, but not msvc. Cap to /O2.
                    "2" | "3" => cmd.args.push("/O2".into()),
                    _ => {}
                }
            }
            ToolFamily::Gnu |
            ToolFamily::Clang => {
                // arm-linux-androideabi-gcc 4.8 shipped with Android NDK does
                // not support '-Oz'
                if opt_level == "z" && cmd.family != ToolFamily::Clang {
                    cmd.args.push("-Os".into());
                } else {
                    cmd.args.push(format!("-O{}", opt_level).into());
                }

                if !nvcc {
                    cmd.args.push("-ffunction-sections".into());
                    cmd.args.push("-fdata-sections".into());
                    if self.pic.unwrap_or(!target.contains("windows-gnu")) {
                        cmd.args.push("-fPIC".into());
                    }
                } else if self.pic.unwrap_or(false) {
                    cmd.args.push("-Xcompiler".into());
                    cmd.args.push("\'-fPIC\'".into());
                }
            }
        }
        for arg in self.envflags(if self.cpp {"CXXFLAGS"} else {"CFLAGS"}) {
            cmd.args.push(arg.into());
        }

        if self.get_debug() {
            cmd.args.push(cmd.family.debug_flag().into());
        }

        // Target flags
        match cmd.family {
            ToolFamily::Clang => {
                cmd.args.push(format!("--target={}", target).into());
            }
            ToolFamily::Msvc => {
                if target.contains("i586") {
                    cmd.args.push("/ARCH:IA32".into());
                }
            }
            ToolFamily::Gnu => {
                if target.contains("i686") || target.contains("i586") {
                    cmd.args.push("-m32".into());
                } else if target.contains("x86_64") || target.contains("powerpc64") {
                    cmd.args.push("-m64".into());
                }

                if self.static_flag.is_none() && target.contains("musl") {
                    cmd.args.push("-static".into());
                }

                // armv7 targets get to use armv7 instructions
                if target.starts_with("armv7-") && target.contains("-linux-") {
                    cmd.args.push("-march=armv7-a".into());
                }

                // On android we can guarantee some extra float instructions
                // (specified in the android spec online)
                if target.starts_with("armv7-linux-androideabi") {
                    cmd.args.push("-march=armv7-a".into());
                    cmd.args.push("-mfpu=vfpv3-d16".into());
                    cmd.args.push("-mfloat-abi=softfp".into());
                }

                // For us arm == armv6 by default
                if target.starts_with("arm-unknown-linux-") {
                    cmd.args.push("-march=armv6".into());
                    cmd.args.push("-marm".into());
                }

                // We can guarantee some settings for FRC
                if target.starts_with("arm-frc-") {
                    cmd.args.push("-march=armv7-a".into());
                    cmd.args.push("-mcpu=cortex-a9".into());
                    cmd.args.push("-mfpu=vfpv3".into());
                    cmd.args.push("-mfloat-abi=softfp".into());
                    cmd.args.push("-marm".into());
                }

                // Turn codegen down on i586 to avoid some instructions.
                if target.starts_with("i586-unknown-linux-") {
                    cmd.args.push("-march=pentium".into());
                }

                // Set codegen level for i686 correctly
                if target.starts_with("i686-unknown-linux-") {
                    cmd.args.push("-march=i686".into());
                }

                // Looks like `musl-gcc` makes is hard for `-m32` to make its way
                // all the way to the linker, so we need to actually instruct the
                // linker that we're generating 32-bit executables as well. This'll
                // typically only be used for build scripts which transitively use
                // these flags that try to compile executables.
                if target == "i686-unknown-linux-musl" {
                    cmd.args.push("-Wl,-melf_i386".into());
                }

                if target.starts_with("thumb") {
                    cmd.args.push("-mthumb".into());

                    if target.ends_with("eabihf") {
                        cmd.args.push("-mfloat-abi=hard".into())
                    }
                }
                if target.starts_with("thumbv6m") {
                    cmd.args.push("-march=armv6s-m".into());
                }
                if target.starts_with("thumbv7em") {
                    cmd.args.push("-march=armv7e-m".into());

                    if target.ends_with("eabihf") {
                        cmd.args.push("-mfpu=fpv4-sp-d16".into())
                    }
                }
                if target.starts_with("thumbv7m") {
                    cmd.args.push("-march=armv7-m".into());
                }
            }
        }

        if target.contains("-ios") {
            // FIXME: potential bug. iOS is always compiled with Clang, but Gcc compiler may be
            // detected instead.
            self.ios_flags(&mut cmd)?;
        }

        if self.static_flag.unwrap_or(false) {
            cmd.args.push("-static".into());
        }
        if self.shared_flag.unwrap_or(false) {
            cmd.args.push("-shared".into());
        }

        if self.cpp {
            match (self.cpp_set_stdlib.as_ref(), cmd.family) {
                (None, _) => { }
                (Some(stdlib), ToolFamily::Gnu) |
                (Some(stdlib), ToolFamily::Clang) => {
                    cmd.args.push(format!("-stdlib=lib{}", stdlib).into());
                }
                _ => {
                    println!("cargo:warning=cpp_set_stdlib is specified, but the {:?} compiler \
                              does not support this option, ignored", cmd.family);
                }
            }
        }

        for directory in self.include_directories.iter() {
            cmd.args.push(cmd.family.include_flag().into());
            cmd.args.push(directory.into());
        }

        for flag in self.flags.iter() {
            cmd.args.push(flag.into());
        }

        for flag in self.flags_supported.iter() {
            if self.is_flag_supported(flag).unwrap_or(false) {
                cmd.args.push(flag.into());
            }
        }

        for &(ref key, ref value) in self.definitions.iter() {
            let lead = if let ToolFamily::Msvc = cmd.family {"/"} else {"-"};
            if let Some(ref value) = *value {
                cmd.args.push(format!("{}D{}={}", lead, key, value).into());
            } else {
                cmd.args.push(format!("{}D{}", lead, key).into());
            }
        }

        if self.warnings {
            for flag in cmd.family.warnings_flags().iter() {
                cmd.args.push(flag.into());
            }
        }

        if self.warnings_into_errors {
            cmd.args.push(cmd.family.warnings_to_errors_flag().into());
        }

        Ok(cmd)
    }

    fn msvc_macro_assembler(&self) -> Result<(Command, String), Error> {
        let target = self.get_target()?;
        let tool = if target.contains("x86_64") {
            "ml64.exe"
        } else {
            "ml.exe"
        };
        let mut cmd = windows_registry::find(&target, tool).unwrap_or_else(|| self.cmd(tool));
        for directory in self.include_directories.iter() {
            cmd.arg("/I").arg(directory);
        }
        for &(ref key, ref value) in self.definitions.iter() {
            if let Some(ref value) = *value {
                cmd.arg(&format!("/D{}={}", key, value));
            } else {
                cmd.arg(&format!("/D{}", key));
            }
        }

        if target.contains("i686") || target.contains("i586") {
            cmd.arg("/safeseh");
        }
        for flag in self.flags.iter() {
            cmd.arg(flag);
        }

        Ok((cmd, tool.to_string()))
    }

    fn assemble(&self, lib_name: &str, dst: &Path, objects: &[PathBuf]) -> Result<(), Error> {
        // Delete the destination if it exists as the `ar` tool at least on Unix
        // appends to it, which we don't want.
        let _ = fs::remove_file(&dst);

        let target = self.get_target()?;
        if target.contains("msvc") {
            let mut cmd = match self.archiver {
                Some(ref s) => self.cmd(s),
                None => windows_registry::find(&target, "lib.exe").unwrap_or_else(|| self.cmd("lib.exe")),
            };
            let mut out = OsString::from("/OUT:");
            out.push(dst);
            run(cmd.arg(out)
                    .arg("/nologo")
                    .args(objects)
                    .args(&self.objects),
                "lib.exe")?;

            // The Rust compiler will look for libfoo.a and foo.lib, but the
            // MSVC linker will also be passed foo.lib, so be sure that both
            // exist for now.
            let lib_dst = dst.with_file_name(format!("{}.lib", lib_name));
            let _ = fs::remove_file(&lib_dst);
            match fs::hard_link(&dst, &lib_dst)
                .or_else(|_| {
                    // if hard-link fails, just copy (ignoring the number of bytes written)
                    fs::copy(&dst, &lib_dst).map(|_| ())
                }) {
                Ok(_) => (),
                Err(_) => return Err(Error::new(ErrorKind::IOError, "Could not copy or create a hard-link to the generated lib file.")),
            };
        } else {
            let ar = self.get_ar()?;
            let cmd = ar.file_name()
                .ok_or_else(|| Error::new(ErrorKind::IOError, "Failed to get archiver (ar) path."))?
                .to_string_lossy();
            run(self.cmd(&ar)
                    .arg("crs")
                    .arg(dst)
                    .args(objects)
                    .args(&self.objects),
                &cmd)?;
        }

        Ok(())
    }

    fn ios_flags(&self, cmd: &mut Tool) -> Result<(), Error> {
        enum ArchSpec {
            Device(&'static str),
            Simulator(&'static str),
        }

        let target = self.get_target()?;
        let arch = target.split('-').nth(0).ok_or_else(|| Error::new(ErrorKind::ArchitectureInvalid, "Unknown architecture for iOS target."))?;
        let arch = match arch {
            "arm" | "armv7" | "thumbv7" => ArchSpec::Device("armv7"),
            "armv7s" | "thumbv7s" => ArchSpec::Device("armv7s"),
            "arm64" | "aarch64" => ArchSpec::Device("arm64"),
            "i386" | "i686" => ArchSpec::Simulator("-m32"),
            "x86_64" => ArchSpec::Simulator("-m64"),
            _ => return Err(Error::new(ErrorKind::ArchitectureInvalid, "Unknown architecture for iOS target.")),
        };

        let sdk = match arch {
            ArchSpec::Device(arch) => {
                cmd.args.push("-arch".into());
                cmd.args.push(arch.into());
                cmd.args.push("-miphoneos-version-min=7.0".into());
                "iphoneos"
            }
            ArchSpec::Simulator(arch) => {
                cmd.args.push(arch.into());
                cmd.args.push("-mios-simulator-version-min=7.0".into());
                "iphonesimulator"
            }
        };

        self.print(&format!("Detecting iOS SDK path for {}", sdk));
        let sdk_path = self.cmd("xcrun")
            .arg("--show-sdk-path")
            .arg("--sdk")
            .arg(sdk)
            .stderr(Stdio::inherit())
            .output()?
            .stdout;

        let sdk_path = match String::from_utf8(sdk_path) {
            Ok(p) => p,
            Err(_) => return Err(Error::new(ErrorKind::IOError, "Unable to determine iOS SDK path.")),
        };

        cmd.args.push("-isysroot".into());
        cmd.args.push(sdk_path.trim().into());

        Ok(())
    }

    fn cmd<P: AsRef<OsStr>>(&self, prog: P) -> Command {
        let mut cmd = Command::new(prog);
        for &(ref a, ref b) in self.env.iter() {
            cmd.env(a, b);
        }
        cmd
    }

    fn get_base_compiler(&self) -> Result<Tool, Error> {
        if let Some(ref c) = self.compiler {
            return Ok(Tool::new(c.clone()));
        }
        let host = self.get_host()?;
        let target = self.get_target()?;
        let (env, msvc, gnu) = if self.cpp {
            ("CXX", "cl.exe", "g++")
        } else {
            ("CC", "cl.exe", "gcc")
        };

        let default = if host.contains("solaris") {
            // In this case, c++/cc unlikely to exist or be correct.
            gnu
        } else if self.cpp {
            "c++"
        } else {
            "cc"
        };

        let tool_opt: Option<Tool> = self.env_tool(env)
            .map(|(tool, args)| {
                let mut t = Tool::new(PathBuf::from(tool));
                for arg in args {
                    t.args.push(arg.into());
                }
                t
            })
            .or_else(|| {
                if target.contains("emscripten") {
                    let tool = if self.cpp {
                        "em++"
                    } else {
                        "emcc"
                    };
                    // Windows uses bat file so we have to be a bit more specific
                    if cfg!(windows) {
                        let mut t = Tool::new(PathBuf::from("cmd"));
                        t.args.push("/c".into());
                        t.args.push(format!("{}.bat", tool).into());
                        Some(t)
                    } else {
                        Some(Tool::new(PathBuf::from(tool)))
                    }
                } else {
                    None
                }
            })
            .or_else(|| windows_registry::find_tool(&target, "cl.exe"));

        let tool = match tool_opt {
            Some(t) => t,
            None => {
                let compiler = if host.contains("windows") && target.contains("windows") {
                    if target.contains("msvc") {
                        msvc.to_string()
                    } else {
                        format!("{}.exe", gnu)
                    }
                } else if target.contains("android") {
                    format!("{}-{}", target.replace("armv7", "arm"), gnu)
                } else if self.get_host()? != target {
                    // CROSS_COMPILE is of the form: "arm-linux-gnueabi-"
                    let cc_env = self.getenv("CROSS_COMPILE");
                    let cross_compile = cc_env.as_ref().map(|s| s.trim_right_matches('-'));
                    let prefix = cross_compile.or(match &target[..] {
                        "aarch64-unknown-linux-gnu" => Some("aarch64-linux-gnu"),
                        "arm-unknown-linux-gnueabi" => Some("arm-linux-gnueabi"),
                        "arm-frc-linux-gnueabi" => Some("arm-frc-linux-gnueabi"),
                        "arm-unknown-linux-gnueabihf" => Some("arm-linux-gnueabihf"),
                        "arm-unknown-linux-musleabi" => Some("arm-linux-musleabi"),
                        "arm-unknown-linux-musleabihf" => Some("arm-linux-musleabihf"),
                        "arm-unknown-netbsd-eabi" => Some("arm--netbsdelf-eabi"),
                        "armv6-unknown-netbsd-eabihf" => Some("armv6--netbsdelf-eabihf"),
                        "armv7-unknown-linux-gnueabihf" => Some("arm-linux-gnueabihf"),
                        "armv7-unknown-linux-musleabihf" => Some("arm-linux-musleabihf"),
                        "armv7-unknown-netbsd-eabihf" => Some("armv7--netbsdelf-eabihf"),
                        "i686-pc-windows-gnu" => Some("i686-w64-mingw32"),
                        "i686-unknown-linux-musl" => Some("musl"),
                        "i686-unknown-netbsd" => Some("i486--netbsdelf"),
                        "mips-unknown-linux-gnu" => Some("mips-linux-gnu"),
                        "mipsel-unknown-linux-gnu" => Some("mipsel-linux-gnu"),
                        "mips64-unknown-linux-gnuabi64" => Some("mips64-linux-gnuabi64"),
                        "mips64el-unknown-linux-gnuabi64" => Some("mips64el-linux-gnuabi64"),
                        "powerpc-unknown-linux-gnu" => Some("powerpc-linux-gnu"),
                        "powerpc-unknown-netbsd" => Some("powerpc--netbsd"),
                        "powerpc64-unknown-linux-gnu" => Some("powerpc-linux-gnu"),
                        "powerpc64le-unknown-linux-gnu" => Some("powerpc64le-linux-gnu"),
                        "s390x-unknown-linux-gnu" => Some("s390x-linux-gnu"),
                        "sparc64-unknown-netbsd" => Some("sparc64--netbsd"),
                        "sparcv9-sun-solaris" => Some("sparcv9-sun-solaris"),
                        "thumbv6m-none-eabi" => Some("arm-none-eabi"),
                        "thumbv7em-none-eabi" => Some("arm-none-eabi"),
                        "thumbv7em-none-eabihf" => Some("arm-none-eabi"),
                        "thumbv7m-none-eabi" => Some("arm-none-eabi"),
                        "x86_64-pc-windows-gnu" => Some("x86_64-w64-mingw32"),
                        "x86_64-rumprun-netbsd" => Some("x86_64-rumprun-netbsd"),
                        "x86_64-unknown-linux-musl" => Some("musl"),
                        "x86_64-unknown-netbsd" => Some("x86_64--netbsd"),
                        _ => None,
                    });
                    match prefix {
                        Some(prefix) => format!("{}-{}", prefix, gnu),
                        None => default.to_string(),
                    }
                } else {
                    default.to_string()
                };
                Tool::new(PathBuf::from(compiler))
            }
        };

        Ok(tool)
    }

    fn get_var(&self, var_base: &str) -> Result<String, Error> {
        let target = self.get_target()?;
        let host = self.get_host()?;
        let kind = if host == target { "HOST" } else { "TARGET" };
        let target_u = target.replace("-", "_");
        let res = self.getenv(&format!("{}_{}", var_base, target))
            .or_else(|| self.getenv(&format!("{}_{}", var_base, target_u)))
            .or_else(|| self.getenv(&format!("{}_{}", kind, var_base)))
            .or_else(|| self.getenv(var_base));

        match res {
            Some(res) => Ok(res),
            None => Err(Error::new(ErrorKind::EnvVarNotFound, &format!("Could not find environment variable {}.", var_base))),
        }
    }

    fn envflags(&self, name: &str) -> Vec<String> {
        self.get_var(name)
            .unwrap_or(String::new())
            .split(|c: char| c.is_whitespace())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }

    fn env_tool(&self, name: &str) -> Option<(String, Vec<String>)> {
        self.get_var(name).ok().map(|tool| {
            let whitelist = ["ccache", "distcc", "sccache"];
            for t in whitelist.iter() {
                if tool.starts_with(t) && tool[t.len()..].starts_with(' ') {
                    return (t.to_string(), vec![tool[t.len()..].trim_left().to_string()]);
                }
            }
            (tool, Vec::new())
        })
    }

    /// Returns the default C++ standard library for the current target: `libc++`
    /// for OS X and `libstdc++` for anything else.
    fn get_cpp_link_stdlib(&self) -> Result<Option<String>, Error> {
        match self.cpp_link_stdlib.clone() {
            Some(s) => Ok(s),
            None => {
                let target = self.get_target()?;
                if target.contains("msvc") {
                    Ok(None)
                } else if target.contains("darwin") {
                    Ok(Some("c++".to_string()))
                } else if target.contains("freebsd") {
                    Ok(Some("c++".to_string()))
                } else {
                    Ok(Some("stdc++".to_string()))
                }
            },
        }
    }

    fn get_ar(&self) -> Result<PathBuf, Error> {
        match self.archiver
            .clone()
            .or_else(|| self.get_var("AR").map(PathBuf::from).ok()) {
                Some(p) => Ok(p),
                None => {
                    if self.get_target()?.contains("android") {
                        Ok(PathBuf::from(format!("{}-ar", self.get_target()?.replace("armv7", "arm"))))
                    } else if self.get_target()?.contains("emscripten") {
                        //Windows use bat files so we have to be a bit more specific
                        let tool = if cfg!(windows) {
                            "emar.bat"
                        } else {
                            "emar"
                        };

                        Ok(PathBuf::from(tool))
                    } else {
                        Ok(PathBuf::from("ar"))
                    }
                }
            }
    }

    fn get_target(&self) -> Result<String, Error> {
        match self.target.clone() {
            Some(t) => Ok(t),
            None => Ok(self.getenv_unwrap("TARGET")?),
        }
    }

    fn get_host(&self) -> Result<String, Error> {
        match self.host.clone() {
            Some(h) => Ok(h),
            None => Ok(self.getenv_unwrap("HOST")?),
        }
    }

    fn get_opt_level(&self) -> Result<String, Error> {
        match self.opt_level.as_ref().cloned() {
            Some(ol) => Ok(ol),
            None => Ok(self.getenv_unwrap("OPT_LEVEL")?),
        }
    }

    fn get_debug(&self) -> bool {
        self.debug.unwrap_or_else(|| {
            match self.getenv("DEBUG") {
                Some(s) => s != "false",
                None => false,
            }
        })
    }

    fn get_out_dir(&self) -> Result<PathBuf, Error> {
        match self.out_dir.clone() {
            Some(p) => Ok(p),
            None => Ok(env::var_os("OUT_DIR")
                .map(PathBuf::from)
                .ok_or_else(|| Error::new(ErrorKind::EnvVarNotFound, "Environment variable OUT_DIR not defined."))?),
        }
    }

    fn getenv(&self, v: &str) -> Option<String> {
        let r = env::var(v).ok();
        self.print(&format!("{} = {:?}", v, r));
        r
    }

    fn getenv_unwrap(&self, v: &str) -> Result<String, Error> {
        match self.getenv(v) {
            Some(s) => Ok(s),
            None => Err(Error::new(ErrorKind::EnvVarNotFound, &format!("Environment variable {} not defined.", v.to_string()))),
        }
    }

    fn print(&self, s: &str) {
        if self.cargo_metadata {
            println!("{}", s);
        }
    }
}

impl Default for Build {
    fn default() -> Build {
        Build::new()
    }
}

impl Tool {
    fn new(path: PathBuf) -> Tool {
        // Try to detect family of the tool from its name, falling back to Gnu.
        let family = if let Some(fname) = path.file_name().and_then(|p| p.to_str()) {
            if fname.contains("clang") {
                ToolFamily::Clang
            } else if fname.contains("cl") && !fname.contains("uclibc") {
                ToolFamily::Msvc
            } else {
                ToolFamily::Gnu
            }
        } else {
            ToolFamily::Gnu
        };
        Tool {
            path: path,
            args: Vec::new(),
            env: Vec::new(),
            family: family
        }
    }

    /// Converts this compiler into a `Command` that's ready to be run.
    ///
    /// This is useful for when the compiler needs to be executed and the
    /// command returned will already have the initial arguments and environment
    /// variables configured.
    pub fn to_command(&self) -> Command {
        let mut cmd = Command::new(&self.path);
        cmd.args(&self.args);
        for &(ref k, ref v) in self.env.iter() {
            cmd.env(k, v);
        }
        cmd
    }

    /// Returns the path for this compiler.
    ///
    /// Note that this may not be a path to a file on the filesystem, e.g. "cc",
    /// but rather something which will be resolved when a process is spawned.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the default set of arguments to the compiler needed to produce
    /// executables for the target this compiler generates.
    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    /// Returns the set of environment variables needed for this compiler to
    /// operate.
    ///
    /// This is typically only used for MSVC compilers currently.
    pub fn env(&self) -> &[(OsString, OsString)] {
        &self.env
    }
}

fn run(cmd: &mut Command, program: &str) -> Result<(), Error> {
    let (mut child, print) = spawn(cmd, program)?;
    let status = match child.wait() {
        Ok(s) => s,
        Err(_) => return Err(Error::new(ErrorKind::ToolExecError, &format!("Failed to wait on spawned child process, command {:?} with args {:?}.", cmd, program))),
    };
    print.join().unwrap();
    println!("{}", status);

    if status.success() {
        Ok(())
    } else {
        Err(Error::new(ErrorKind::ToolExecError, &format!("Command {:?} with args {:?} did not execute successfully (status code {}).", cmd, program, status)))
    }
}

fn run_output(cmd: &mut Command, program: &str) -> Result<Vec<u8>, Error> {
    cmd.stdout(Stdio::piped());
    let (mut child, print) = spawn(cmd, program)?;
    let mut stdout = vec![];
    child.stdout.take().unwrap().read_to_end(&mut stdout).unwrap();
    let status = match child.wait() {
        Ok(s) => s,
        Err(_) => return Err(Error::new(ErrorKind::ToolExecError, &format!("Failed to wait on spawned child process, command {:?} with args {:?}.", cmd, program))),
    };
    print.join().unwrap();
    println!("{}", status);

    if status.success() {
        Ok(stdout)
    } else {
        Err(Error::new(ErrorKind::ToolExecError, &format!("Command {:?} with args {:?} did not execute successfully (status code {}).", cmd, program, status)))
    }
}

fn spawn(cmd: &mut Command, program: &str) -> Result<(Child, JoinHandle<()>), Error> {
    println!("running: {:?}", cmd);

    // Capture the standard error coming from these programs, and write it out
    // with cargo:warning= prefixes. Note that this is a bit wonky to avoid
    // requiring the output to be UTF-8, we instead just ship bytes from one
    // location to another.
    match cmd.stderr(Stdio::piped()).spawn() {
        Ok(mut child) => {
            let stderr = BufReader::new(child.stderr.take().unwrap());
            let print = thread::spawn(move || {
                for line in stderr.split(b'\n').filter_map(|l| l.ok()) {
                    print!("cargo:warning=");
                    std::io::stdout().write_all(&line).unwrap();
                    println!("");
                }
            });
            Ok((child, print))
        }
        Err(ref e) if e.kind() == io::ErrorKind::NotFound => {
            let extra = if cfg!(windows) {
                " (see https://github.com/alexcrichton/gcc-rs#compile-time-requirements \
                   for help)"
            } else {
                ""
            };
            Err(Error::new(ErrorKind::ToolNotFound, &format!("Failed to find tool. Is `{}` installed?{}", program, extra)))
        }
        Err(_) => Err(Error::new(ErrorKind::ToolExecError, &format!("Command {:?} with args {:?} failed to start.", cmd, program))),
    }
}

fn fail(s: &str) -> ! {
    panic!("\n\nInternal error occurred: {}\n\n", s)
}


fn command_add_output_file(cmd: &mut Command, dst: &Path, msvc: bool, is_asm: bool) {
    if msvc && is_asm {
        cmd.arg("/Fo").arg(dst);
    } else if msvc {
        let mut s = OsString::from("/Fo");
        s.push(&dst);
        cmd.arg(s);
    } else {
        cmd.arg("-o").arg(&dst);
    }
}
