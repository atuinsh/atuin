//! How to extract subcommands' args into external structs.

use structopt::StructOpt;

#[derive(Debug, StructOpt)]
pub struct Foo {
    pub bar: Option<String>,
}

#[derive(Debug, StructOpt)]
pub enum Command {
    #[structopt(name = "foo")]
    Foo(Foo),
}

#[derive(Debug, StructOpt)]
#[structopt(name = "classify")]
pub struct ApplicationArguments {
    #[structopt(subcommand)]
    pub command: Command,
}

fn main() {
    let opt = ApplicationArguments::from_args();
    println!("{:?}", opt);
}
