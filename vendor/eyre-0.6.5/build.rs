use std::env;
use std::fs;
use std::path::Path;
use std::process::{Command, ExitStatus};

// This code exercises the surface area that we expect of the std Backtrace
// type. If the current toolchain is able to compile it, we go ahead and use
// backtrace in eyre.
const BACKTRACE_PROBE: &str = r#"
    #![feature(backtrace)]
    #![allow(dead_code)]

    use std::backtrace::{Backtrace, BacktraceStatus};
    use std::error::Error;
    use std::fmt::{self, Display};

    #[derive(Debug)]
    struct E;

    impl Display for E {
        fn fmt(&self, _formatter: &mut fmt::Formatter) -> fmt::Result {
            unimplemented!()
        }
    }

    impl Error for E {
        fn backtrace(&self) -> Option<&Backtrace> {
            let backtrace = Backtrace::capture();
            match backtrace.status() {
                BacktraceStatus::Captured | BacktraceStatus::Disabled | _ => {}
            }
            unimplemented!()
        }
    }
"#;

const TRACK_CALLER_PROBE: &str = r#"
    #![allow(dead_code)]

    #[track_caller]
    fn foo() {
        let _location = std::panic::Location::caller();
    }
"#;

fn main() {
    match compile_probe(BACKTRACE_PROBE) {
        Some(status) if status.success() => println!("cargo:rustc-cfg=backtrace"),
        _ => {}
    }

    match compile_probe(TRACK_CALLER_PROBE) {
        Some(status) if status.success() => println!("cargo:rustc-cfg=track_caller"),
        _ => {}
    }
}

fn compile_probe(probe: &str) -> Option<ExitStatus> {
    let rustc = env::var_os("RUSTC")?;
    let out_dir = env::var_os("OUT_DIR")?;
    let probefile = Path::new(&out_dir).join("probe.rs");
    fs::write(&probefile, probe).ok()?;
    Command::new(rustc)
        .arg("--edition=2018")
        .arg("--crate-name=eyre_build")
        .arg("--crate-type=lib")
        .arg("--emit=metadata")
        .arg("--out-dir")
        .arg(out_dir)
        .arg(probefile)
        .status()
        .ok()
}
