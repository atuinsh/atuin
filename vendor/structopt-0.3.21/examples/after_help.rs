//! How to append a postscript to the help message generated.

use structopt::StructOpt;

/// I am a program and I do things.
///
/// Sometimes they even work.
#[derive(StructOpt, Debug)]
#[structopt(after_help = "Beware `-d`, dragons be here")]
struct Opt {
    /// Release the dragon.
    #[structopt(short)]
    dragon: bool,
}

fn main() {
    let opt = Opt::from_args();
    println!("{:?}", opt);
}
