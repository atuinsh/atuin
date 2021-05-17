extern crate scanlex;

fn main() {
    let def = "10 0.1 0.0 + 1.0e4 1e-3-5+4 0.1e+2";
    let text = std::env::args().skip(1).next().unwrap_or(def.to_string());
    let scan = scanlex::Scanner::new(&text);
    for t in scan {
        println!("{:?}",t);
    }   
}
