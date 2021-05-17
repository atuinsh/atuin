//! How to use `arg_enum!` with `StructOpt`.

use clap::arg_enum;
use structopt::StructOpt;

arg_enum! {
    #[derive(Debug)]
    enum Baz {
        Foo,
        Bar,
        FooBar
    }
}

#[derive(StructOpt, Debug)]
struct Opt {
    /// Important argument.
    #[structopt(possible_values = &Baz::variants(), case_insensitive = true)]
    i: Baz,
}

fn main() {
    let opt = Opt::from_args();
    println!("{:?}", opt);
}
