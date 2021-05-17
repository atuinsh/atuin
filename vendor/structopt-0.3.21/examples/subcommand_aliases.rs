//! How to assign some aliases to subcommands

use structopt::clap::AppSettings;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
// https://docs.rs/clap/2/clap/enum.AppSettings.html#variant.InferSubcommands
#[structopt(setting = AppSettings::InferSubcommands)]
enum Opt {
    // https://docs.rs/clap/2/clap/struct.App.html#method.alias
    #[structopt(alias = "foobar")]
    Foo,
    // https://docs.rs/clap/2/clap/struct.App.html#method.aliases
    #[structopt(aliases = &["baz", "fizz"])]
    Bar,
}

fn main() {
    let opt = Opt::from_args();
    println!("{:?}", opt);
}
