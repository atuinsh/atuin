use std::hint::black_box;
use std::path::PathBuf;
use std::process::Command;

use brunch::{Bench, Benches};
use fuzzy_matcher::FuzzyMatcher;
use nucleo::{Utf32Str, Utf32String};

fn bench_dir() -> PathBuf {
    std::env::var_os("BENCHMARK_DIR")
        .expect("the BENCHMARK_DIR must be set to the directory to traverse for the benchmark")
        .into()
}

fn checkout_linux_if_needed() {
    let linux_dir = bench_dir();
    if !linux_dir.exists() {
        println!("will git clone linux...");
        let output = Command::new("git")
            .arg("clone")
            .arg("https://github.com/BurntSushi/linux.git")
            .arg("--depth")
            .arg("1")
            .arg("--branch")
            .arg("master")
            .arg("--single-branch")
            .arg(&linux_dir)
            .stdout(std::process::Stdio::inherit())
            .status()
            .expect("failed to git clone linux");
        println!("did git clone linux...{:?}", output);
    }
}

fn main() {
    checkout_linux_if_needed();
    let dir = bench_dir();
    let paths: (Vec<Utf32String>, Vec<String>) = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|path| {
            let dent = path.ok()?;
            let path = dent.into_path().to_string_lossy().into_owned();
            Some((path.as_str().into(), path))
        })
        .unzip();
    let mut nucleo = nucleo::Matcher::new(nucleo::Config::DEFAULT.match_paths());
    let skim = fuzzy_matcher::skim::SkimMatcherV2::default();

    // TODO: unicode?
    let needles = ["never_matches", "copying", "/doc/kernel", "//.h"];
    // Announce that we've started.
    ::std::eprint!("\x1b[1;38;5;199mStarting:\x1b[0m Running benchmark(s). Stand by!\n\n");
    let mut benches = Benches::default();
    // let mut scores = Vec::with_capacity(paths.0.len());
    for needle in needles {
        println!("running {needle:?}...");
        benches.push(Bench::new(format!("nucleo {needle:?}")).run(|| {
            // scores.clear();
            // scores.extend(paths.0.iter().filter_map(|haystack| {
            for haystack in &paths.0 {
                black_box(
                    nucleo.fuzzy_match(haystack.slice(..), Utf32Str::Ascii(needle.as_bytes())),
                );
            }
            // }));
            // scores.sort_unstable();
        }));
        benches.push(Bench::new(format!("skim {needle:?}")).run(|| {
            for haystack in &paths.1 {
                let res = skim.fuzzy_match(haystack, needle);
                let _ = black_box(res);
            }
        }));
    }
    benches.finish();
}
