use proc_macro_hack::proc_macro_hack;

#[proc_macro_hack]
fn my_macro(input: TokenStream) -> TokenStream {
    unimplemented!()
}

fn main() {}
