use proc_macro_hack::proc_macro_hack;

#[proc_macro_hack(fake_call_site, support_nexted)]
pub use demo::some_macro;

fn main() {}
