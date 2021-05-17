//! How to use flattening.

use structopt::StructOpt;

#[derive(StructOpt, Debug)]
struct Cmdline {
    /// switch verbosity on
    #[structopt(short)]
    verbose: bool,

    #[structopt(flatten)]
    daemon_opts: DaemonOpts,
}

#[derive(StructOpt, Debug)]
struct DaemonOpts {
    /// daemon user
    #[structopt(short)]
    user: String,

    /// daemon group
    #[structopt(short)]
    group: String,
}

fn main() {
    let opt = Cmdline::from_args();
    println!("{:?}", opt);
}
