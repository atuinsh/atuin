//! How to require presence of at least N values,
//! like `val1 val2 ... valN ... valM`.

use structopt::StructOpt;

#[derive(StructOpt, Debug)]
struct Opt {
    #[structopt(required = true, min_values = 2)]
    foos: Vec<String>,
}

fn main() {
    let opt = Opt::from_args();
    println!("{:?}", opt);
}
