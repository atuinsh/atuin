//! How to add `no-thing` flag which is `true` by default and
//! `false` if passed.

use structopt::StructOpt;

#[derive(Debug, StructOpt)]
struct Opt {
    #[structopt(long = "no-verbose", parse(from_flag = std::ops::Not::not))]
    verbose: bool,
}

fn main() {
    let cmd = Opt::from_args();
    println!("{:#?}", cmd);
}
