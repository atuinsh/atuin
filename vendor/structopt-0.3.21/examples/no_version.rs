//! How to completely remove version.

use structopt::clap::AppSettings;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
#[structopt(
    name = "no_version",
    no_version,
    global_settings = &[AppSettings::DisableVersion]
)]
struct Opt {}

fn main() {
    let opt = Opt::from_args();
    println!("{:?}", opt);
}
