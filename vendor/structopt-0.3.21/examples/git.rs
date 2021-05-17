//! `git.rs` serves as a demonstration of how to use subcommands,
//! as well as a demonstration of adding documentation to subcommands.
//! Documentation can be added either through doc comments or
//! `help`/`about` attributes.

use structopt::StructOpt;

#[derive(StructOpt, Debug)]
#[structopt(name = "git")]
/// the stupid content tracker
enum Opt {
    /// fetch branches from remote repository
    Fetch {
        #[structopt(long)]
        dry_run: bool,
        #[structopt(long)]
        all: bool,
        #[structopt(default_value = "origin")]
        repository: String,
    },
    #[structopt(help = "add files to the staging area")]
    Add {
        #[structopt(short)]
        interactive: bool,
        #[structopt(short)]
        all: bool,
        files: Vec<String>,
    },
}

fn main() {
    let matches = Opt::from_args();

    println!("{:?}", matches);
}
