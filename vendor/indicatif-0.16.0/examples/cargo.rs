use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use console::{Style, Term};
use indicatif::{HumanDuration, ProgressBar, ProgressStyle};
use rand::Rng;

static CRATES: &[(&str, &str)] = &[
    ("console", "v0.14.1"),
    ("lazy_static", "v1.4.0"),
    ("libc", "v0.2.93"),
    ("regex", "v1.4.6"),
    ("regex-syntax", "v0.6.23"),
    ("terminal_size", "v0.1.16"),
    ("libc", "v0.2.93"),
    ("unicode-width", "v0.1.8"),
    ("lazy_static", "v1.4.0"),
    ("number_prefix", "v0.4.0"),
    ("regex", "v1.4.6"),
    ("rand", "v0.8.3"),
    ("getrandom", "v0.2.2"),
    ("cfg-if", "v1.0.0"),
    ("libc", "v0.2.93"),
    ("rand_chacha", "v0.3.0"),
    ("ppv-lite86", "v0.2.10"),
    ("rand_core", "v0.6.2"),
    ("getrandom", "v0.2.2"),
    ("rand_core", "v0.6.2"),
    ("tokio", "v1.5.0"),
    ("bytes", "v1.0.1"),
    ("pin-project-lite", "v0.2.6"),
    ("slab", "v0.4.3"),
    ("indicatif", "v0.15.0"),
];

fn main() {
    // number of cpus
    const NUM_CPUS: usize = 4;
    let start = Instant::now();

    // mimic cargo progress bar although it behaves a bit different
    let pb = ProgressBar::new(CRATES.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            // note that bar size is fixed unlike cargo which is dynamic
            // and also the truncation in cargo uses trailers (`...`)
            .template(if Term::stdout().size().1 > 80 {
                "{prefix:>12.cyan.bold} [{bar:57}] {pos}/{len} {wide_msg}"
            } else {
                "{prefix:>12.cyan.bold} [{bar:57}] {pos}/{len}"
            })
            .progress_chars("=> "),
    );
    pb.set_prefix("Building");

    // process in another thread
    // crates to be iterated but not exactly a tree
    let crates = Arc::new(Mutex::new(CRATES.iter()));
    let (tx, rx) = mpsc::channel();
    for n in 0..NUM_CPUS {
        let tx = tx.clone();
        let crates = crates.clone();
        thread::spawn(move || {
            let mut rng = rand::thread_rng();
            loop {
                let krate = crates.lock().unwrap().next();
                // notify main thread if n thread is processing a crate
                tx.send((n, krate)).unwrap();
                if let Some(krate) = krate {
                    thread::sleep(Duration::from_millis(
                        // last compile and linking is always slow, let's mimic that
                        if CRATES.last() == Some(krate) {
                            rng.gen_range(1_000..2_000)
                        } else {
                            rng.gen_range(250..1_000)
                        },
                    ));
                } else {
                    break;
                }
            }
        });
    }
    // drop tx to stop waiting
    drop(tx);

    let green_bold = Style::new().green().bold();

    // do progress drawing in main thread
    let mut processing = vec![None; NUM_CPUS];
    while let Ok((n, krate)) = rx.recv() {
        processing[n] = krate;
        let crates: Vec<&str> = processing
            .iter()
            .filter_map(|t| t.copied().map(|(name, _)| name))
            .collect();
        pb.set_message(crates.join(", "));
        if let Some((name, version)) = krate {
            // crate is being built
            let line = format!(
                "{:>12} {} {}",
                green_bold.apply_to("Compiling"),
                name,
                version
            );
            pb.println(line);

            pb.inc(1);
        }
    }
    pb.finish_and_clear();

    // compilation is finished
    println!(
        "{:>12} dev [unoptimized + debuginfo] target(s) in {}",
        green_bold.apply_to("Finished"),
        HumanDuration(start.elapsed())
    );
}
