#![forbid(unsafe_code)]
extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use slugify::slugify;
use syn::parse::Parser;

fn literal_to_slug(literal: &syn::ExprLit) -> TokenStream2 {
    // We pull out the actual text from the literal string expression.
    let quoted: String = match &literal.lit {
        syn::Lit::Str(message_id) => message_id.value(),
        _ => panic!("Message ID must be a literal string"),
    };
    // ...and pass it to slugify,
    let slug = slugify!(quoted.as_str());
    // ...then turn it back into a literal string.
    quote!(#slug)
}

#[proc_macro]
pub fn tl(tokens: TokenStream) -> TokenStream {
    // Begin by getting the individual arguments to tl!
    let args = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated
        .parse(tokens)
        .unwrap();

    let mut arg_iter = args.iter();

    // The first should always be the fl! macro for fluent (cf. atuin-common/src/i18n.rs)
    let fl = arg_iter.next().unwrap();
    // atuin-common will send the universal loader as the second argument. This avoids
    // every translation string having to explicitly pass it.
    let loader = arg_iter.next().unwrap();

    // The third argument should be the message ID. This logic takes the human-readable
    // string and slugifies it. One of the main benefits of Fluent is that English-language
    // ASCII is not the de facto reference (and things like gender and plurality can be
    // encoded even where English makes no grammatical distinction). However, this approach
    // still allows the `fl!` macro to be used directly, but saves having to switch all
    // strings to slugs throughout the codebase just to make them translatable at all.

    // It is possible that the string literal representing the message (e.g. "Danger, Bill Bobinson")
    // appears wrapped in a group or not, so we handle both possibilities.
    // We use literal_to_slug to turn it to a slug, e.g. "danger-bill-bobinson"
    let message_id: proc_macro2::TokenStream = match arg_iter.next() {
        Some(syn::Expr::Group(arg)) => match *arg.expr.clone() {
            syn::Expr::Lit(arg) => literal_to_slug(&arg),
            arg => panic!("Message ID {:?} must be a literal", arg),
        },
        Some(syn::Expr::Lit(arg)) => literal_to_slug(arg),
        arg => panic!("Message ID {:?} must be a literal", arg),
    };

    // Reconstruct the arguments that we initially had, and pull in any extra ones
    // that should go right through to Fluent. For example:
    //     t!("Danger ${name}", name="Bill Bobinson")
    //     -> tr!(fl, LOADER, "Danger ${name}", name="Bill Bobinson")
    //     -> fl!(LOADER, "danger-name", name="Bill Bobinson")
    // `danger-name` is then searched for in the i18n/ folder, and should map
    // to a template like `Danger, { $name }` that Fluent can insert the parameter into.
    let args: Vec<_> = arg_iter.collect();

    // If there are no parameters, then Fluent can do this entirely statically.
    // Otherwise, it will require runtime interpolation.
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
