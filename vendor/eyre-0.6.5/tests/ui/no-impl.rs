use eyre::eyre;

#[derive(Debug)]
struct Error;

fn main() {
    let _ = eyre!(Error);
}
