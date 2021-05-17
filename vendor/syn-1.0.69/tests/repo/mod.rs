mod progress;

use self::progress::Progress;
use anyhow::Result;
use flate2::read::GzDecoder;
use std::fs;
use std::path::Path;
use tar::Archive;
use walkdir::DirEntry;

const REVISION: &str = "52e3dffa50cfffdcfa145c0cc0ba48b49abc0c07";

#[rustfmt::skip]
static EXCLUDE: &[&str] = &[
    // Compile-fail expr parameter in const generic position: f::<1 + 2>()
    "src/test/ui/const-generics/closing-args-token.rs",
    "src/test/ui/const-generics/const-expression-parameter.rs",

    // Deprecated anonymous parameter syntax in traits
    "src/test/ui/issues/issue-13105.rs",
    "src/test/ui/issues/issue-13775.rs",
    "src/test/ui/issues/issue-34074.rs",
    "src/test/ui/proc-macro/trait-fn-args-2015.rs",

    // Not actually test cases
    "src/test/rustdoc-ui/test-compile-fail2.rs",
    "src/test/rustdoc-ui/test-compile-fail3.rs",
    "src/test/ui/include-single-expr-helper.rs",
    "src/test/ui/include-single-expr-helper-1.rs",
    "src/test/ui/json-bom-plus-crlf-multifile-aux.rs",
    "src/test/ui/lint/expansion-time-include.rs",
    "src/test/ui/macros/auxiliary/macro-comma-support.rs",
    "src/test/ui/macros/auxiliary/macro-include-items-expr.rs",
    "src/test/ui/parser/auxiliary/issue-21146-inc.rs",
];

pub fn base_dir_filter(entry: &DirEntry) -> bool {
    let path = entry.path();
    if path.is_dir() {
        return true; // otherwise walkdir does not visit the files
    }
    if path.extension().map(|e| e != "rs").unwrap_or(true) {
        return false;
    }

    let mut path_string = path.to_string_lossy();
    if cfg!(windows) {
        path_string = path_string.replace('\\', "/").into();
    }
    let path = if let Some(path) = path_string.strip_prefix("tests/rust/") {
        path
    } else {
        panic!("unexpected path in Rust dist: {}", path_string);
    };

    if path.starts_with("src/test/compile-fail") || path.starts_with("src/test/rustfix") {
        return false;
    }

    if path.starts_with("src/test/ui") {
        let stderr_path = entry.path().with_extension("stderr");
        if stderr_path.exists() {
            // Expected to fail in some way
            return false;
        }
    }

    !EXCLUDE.contains(&path)
}

#[allow(dead_code)]
pub fn edition(path: &Path) -> &'static str {
    if path.ends_with("dyn-2015-no-warnings-without-lints.rs") {
        "2015"
    } else {
        "2018"
    }
}

pub fn clone_rust() {
    let needs_clone = match fs::read_to_string("tests/rust/COMMIT") {
        Err(_) => true,
        Ok(contents) => contents.trim() != REVISION,
    };
    if needs_clone {
        download_and_unpack().unwrap();
    }
    let mut missing = String::new();
    let test_src = Path::new("tests/rust");
    for exclude in EXCLUDE {
        if !test_src.join(exclude).exists() {
            missing += "\ntests/rust/";
            missing += exclude;
        }
    }
    if !missing.is_empty() {
        panic!("excluded test file does not exist:{}\n", missing);
    }
}

fn download_and_unpack() -> Result<()> {
    let url = format!(
        "https://github.com/rust-lang/rust/archive/{}.tar.gz",
        REVISION
    );
    let response = reqwest::blocking::get(&url)?.error_for_status()?;
    let progress = Progress::new(response);
    let decoder = GzDecoder::new(progress);
    let mut archive = Archive::new(decoder);
    let prefix = format!("rust-{}", REVISION);

    let tests_rust = Path::new("tests/rust");
    if tests_rust.exists() {
        fs::remove_dir_all(tests_rust)?;
    }

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if path == Path::new("pax_global_header") {
            continue;
        }
        let relative = path.strip_prefix(&prefix)?;
        let out = tests_rust.join(relative);
        entry.unpack(&out)?;
    }

    fs::write("tests/rust/COMMIT", REVISION)?;
    Ok(())
}
