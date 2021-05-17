use lazy_static::lazy_static;
use once_cell::sync::Lazy;

const N_THREADS: usize = 32;
const N_ROUNDS: usize = 100_000_000;

static ONCE_CELL: Lazy<Vec<String>> = Lazy::new(|| vec!["Spica".to_string(), "Hoyten".to_string()]);

lazy_static! {
    static ref LAZY_STATIC: Vec<String> = vec!["Spica".to_string(), "Hoyten".to_string()];
}

fn main() {
    let once_cell = {
        let start = std::time::Instant::now();
        let threads = (0..N_THREADS)
            .map(|_| std::thread::spawn(move || thread_once_cell()))
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        start.elapsed()
    };
    let lazy_static = {
        let start = std::time::Instant::now();
        let threads = (0..N_THREADS)
            .map(|_| std::thread::spawn(move || thread_lazy_static()))
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        start.elapsed()
    };

    println!("once_cell:   {:?}", once_cell);
    println!("lazy_static: {:?}", lazy_static);
}

fn thread_once_cell() {
    for _ in 0..N_ROUNDS {
        let len = ONCE_CELL.len();
        assert_eq!(len, 2)
    }
}

fn thread_lazy_static() {
    for _ in 0..N_ROUNDS {
        let len = LAZY_STATIC.len();
        assert_eq!(len, 2)
    }
}
