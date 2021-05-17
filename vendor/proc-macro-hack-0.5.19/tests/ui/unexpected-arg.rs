use proc_macro_hack::proc_macro_hack;

#[proc_macro_hack(fake_call_site)]
pub fn my_macro(input: TokenStream) -> TokenStream {
    unimplemented!()
}

fn main() {}
