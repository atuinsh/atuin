use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use lazy_static::lazy_static;
use rand::{rngs::ThreadRng, Rng, RngCore};
use std::fmt::Debug;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
enum Action {
    ModifyTree(usize),
    IncProgressBar(usize),
    Stop,
}

#[derive(Clone, Debug)]
enum Elem {
    AddItem(Item),
    RemoveItem(Index),
}

#[derive(Clone, Debug)]
struct Item {
    key: String,
    index: usize,
    indent: usize,
    progress_bar: ProgressBar,
}

#[derive(Clone, Debug)]
struct Index(usize);

const PB_LEN: u64 = 32;
static ELEM_IDX: AtomicUsize = AtomicUsize::new(0);

lazy_static! {
    static ref ELEMENTS: [Elem; 27] = [
        Elem::AddItem(Item {
            indent: 9,
            index: 0,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "dog".to_string()
        }),
        Elem::AddItem(Item {
            indent: 0,
            index: 0,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "temp_1".to_string()
        }),
        Elem::AddItem(Item {
            indent: 8,
            index: 1,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "lazy".to_string()
        }),
        Elem::AddItem(Item {
            indent: 0,
            index: 1,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "temp_2".to_string()
        }),
        Elem::AddItem(Item {
            indent: 1,
            index: 0,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "the".to_string()
        }),
        Elem::AddItem(Item {
            indent: 0,
            index: 0,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "temp_3".to_string()
        }),
        Elem::AddItem(Item {
            indent: 7,
            index: 3,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "a".to_string()
        }),
        Elem::AddItem(Item {
            indent: 0,
            index: 3,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "temp_4".to_string()
        }),
        Elem::AddItem(Item {
            indent: 6,
            index: 2,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "over".to_string()
        }),
        Elem::RemoveItem(Index(6)),
        Elem::RemoveItem(Index(4)),
        Elem::RemoveItem(Index(3)),
        Elem::RemoveItem(Index(0)),
        Elem::AddItem(Item {
            indent: 0,
            index: 2,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "temp_5".to_string()
        }),
        Elem::AddItem(Item {
            indent: 4,
            index: 1,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "fox".to_string()
        }),
        Elem::AddItem(Item {
            indent: 0,
            index: 1,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "temp_6".to_string()
        }),
        Elem::AddItem(Item {
            indent: 2,
            index: 1,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "quick".to_string()
        }),
        Elem::AddItem(Item {
            indent: 0,
            index: 1,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "temp_7".to_string()
        }),
        Elem::AddItem(Item {
            indent: 5,
            index: 5,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "jumps".to_string()
        }),
        Elem::AddItem(Item {
            indent: 0,
            index: 5,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "temp_8".to_string()
        }),
        Elem::AddItem(Item {
            indent: 3,
            index: 4,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "brown".to_string()
        }),
        Elem::AddItem(Item {
            indent: 0,
            index: 3,
            progress_bar: ProgressBar::new(PB_LEN),
            key: "temp_9".to_string()
        }),
        Elem::RemoveItem(Index(10)),
        Elem::RemoveItem(Index(7)),
        Elem::RemoveItem(Index(4)),
        Elem::RemoveItem(Index(3)),
        Elem::RemoveItem(Index(1)),
    ];
}

/// The example demonstrates the usage of `MultiProgress` and further extends `multi-tree` example.
/// Now the example has 3 different actions implemented, and the item tree can be modified
/// by inserting or removing progress bars. The progress bars to be removed eventually
/// have messages with pattern `"temp_*"`.
pub fn main() {
    let mp = Arc::new(MultiProgress::new());
    let sty_main = ProgressStyle::default_bar().template("{bar:40.green/yellow} {pos:>4}/{len:4}");
    let sty_aux =
        ProgressStyle::default_bar().template("[{pos:>2}/{len:2}] {prefix}{spinner:.green} {msg}");
    let sty_fin = ProgressStyle::default_bar().template("[{pos:>2}/{len:2}] {prefix}{msg}");

    let pb_main = mp.add(ProgressBar::new(
        ELEMENTS
            .iter()
            .map(|e| match e {
                Elem::AddItem(item) => item.progress_bar.length(),
                Elem::RemoveItem(_) => 1,
            })
            .sum(),
    ));

    pb_main.set_style(sty_main);
    for e in ELEMENTS.iter() {
        match e {
            Elem::AddItem(item) => item.progress_bar.set_style(sty_aux.clone()),
            Elem::RemoveItem(_) => {}
        }
    }

    let mut items: Vec<&Item> = Vec::with_capacity(ELEMENTS.len());

    let mp2 = Arc::clone(&mp);
    let _ = thread::spawn(move || {
        let mut rng = ThreadRng::default();
        pb_main.tick();
        loop {
            match get_action(&mut rng, &items) {
                Action::Stop => {
                    // all elements were exhausted
                    pb_main.finish();
                    return;
                }
                Action::ModifyTree(elem_idx) => match &ELEMENTS[elem_idx] {
                    Elem::AddItem(item) => {
                        let pb = mp2.insert(item.index + 1, item.progress_bar.clone());
                        pb.set_prefix("  ".repeat(item.indent));
                        pb.set_message(&item.key);
                        items.insert(item.index, &item);
                    }
                    Elem::RemoveItem(Index(index)) => {
                        let item = items.remove(*index);
                        let pb = &item.progress_bar;
                        mp2.remove(pb);
                        pb_main.inc(pb.length() - pb.position());
                    }
                },
                Action::IncProgressBar(item_idx) => {
                    let item = &items[item_idx];
                    item.progress_bar.inc(1);
                    let pos = item.progress_bar.position();
                    let len = item.progress_bar.length();
                    if pos >= len {
                        item.progress_bar.set_style(sty_fin.clone());
                        item.progress_bar.finish_with_message(format!(
                            "{} {}",
                            style("✔").green(),
                            item.key
                        ));
                    }
                    pb_main.inc(1);
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
    });

    mp.join().unwrap();
}

/// The function guarantees to return the action, that is valid for the current tree.
fn get_action<'a>(rng: &'a mut dyn RngCore, items: &[&Item]) -> Action {
    let elem_idx = ELEM_IDX.load(Ordering::SeqCst);
    // the indices of those items, that not completed yet
    let uncompleted = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            let pos = item.progress_bar.position();
            let len = item.progress_bar.length();
            pos < len
        })
        .map(|(idx, _)| idx)
        .collect::<Vec<usize>>();
    let k = rng.gen_range(0..16);
    if (k > 0 || k == 0 && elem_idx == ELEMENTS.len()) && !uncompleted.is_empty() {
        let idx = rng.gen_range(0..uncompleted.len() as u64) as usize;
        Action::IncProgressBar(uncompleted[idx])
    } else if elem_idx < ELEMENTS.len() {
        ELEM_IDX.fetch_add(1, Ordering::SeqCst);
        Action::ModifyTree(elem_idx)
    } else {
        // nothing to do more
        Action::Stop
    }
}
