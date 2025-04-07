#![forbid(unsafe_code)]
extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn;
use syn::parse::Parser;
use slugify::slugify;

#[proc_macro]
pub fn tl(tokens: TokenStream) -> TokenStream {
    let args = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated.parse(tokens).unwrap();
    let mut arg_iter = args.iter();
    let fl = arg_iter.next().unwrap();
    let loader = arg_iter.next().unwrap();
    let message_id: proc_macro2::TokenStream = match arg_iter.next() {
        Some(syn::Expr::Group(arg)) => {
            match *arg.expr.clone() {
                syn::Expr::Lit(arg) => {
                    let quoted: String = match arg.lit.clone() {
                        syn::Lit::Str(message_id) => message_id.value(),
                        _ => panic!("Message ID must be a literal string")
                    };
                    let slug = slugify!(quoted.as_str());
                    quote!(#slug)
                },
                arg => panic!("Message ID {:?} must be a literal", arg)
            }
        },
        Some(syn::Expr::Lit(arg)) => {
            let quoted: String = match arg.lit.clone() {
                syn::Lit::Str(message_id) => message_id.value(),
                _ => panic!("Message ID must be a literal string")
            };
            let slug = slugify!(quoted.as_str());
            quote!(#slug)
        },
        arg => panic!("Message ID {:?} must be a literal", arg)
    };
    let args: Vec<_> = arg_iter.collect();

    if args.is_empty() {
        TokenStream::from(quote!(
            #fl!(#loader, #message_id)
        ))
    } else {
        TokenStream::from(quote!(
            #fl!(#loader, #message_id, #(#args),*)
        ))
    }
}

