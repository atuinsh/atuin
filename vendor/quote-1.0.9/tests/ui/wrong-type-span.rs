use quote::quote_spanned;

fn main() {
    let span = "";
    let x = 0;
    quote_spanned!(span=> #x);
}
