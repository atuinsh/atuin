//! [![github]](https://github.com/dtolnay/proc-macro-hack)&ensp;[![crates-io]](https://crates.io/crates/proc-macro-hack)&ensp;[![docs-rs]](https://docs.rs/proc-macro-hack)
//!
//! [github]: https://img.shields.io/badge/github-8da0cb?style=for-the-badge&labelColor=555555&logo=github
//! [crates-io]: https://img.shields.io/badge/crates.io-fc8d62?style=for-the-badge&labelColor=555555&logo=rust
//! [docs-rs]: https://img.shields.io/badge/docs.rs-66c2a5?style=for-the-badge&labelColor=555555&logoColor=white&logo=data:image/svg+xml;base64,PHN2ZyByb2xlPSJpbWciIHhtbG5zPSJodHRwOi8vd3d3LnczLm9yZy8yMDAwL3N2ZyIgdmlld0JveD0iMCAwIDUxMiA1MTIiPjxwYXRoIGZpbGw9IiNmNWY1ZjUiIGQ9Ik00ODguNiAyNTAuMkwzOTIgMjE0VjEwNS41YzAtMTUtOS4zLTI4LjQtMjMuNC0zMy43bC0xMDAtMzcuNWMtOC4xLTMuMS0xNy4xLTMuMS0yNS4zIDBsLTEwMCAzNy41Yy0xNC4xIDUuMy0yMy40IDE4LjctMjMuNCAzMy43VjIxNGwtOTYuNiAzNi4yQzkuMyAyNTUuNSAwIDI2OC45IDAgMjgzLjlWMzk0YzAgMTMuNiA3LjcgMjYuMSAxOS45IDMyLjJsMTAwIDUwYzEwLjEgNS4xIDIyLjEgNS4xIDMyLjIgMGwxMDMuOS01MiAxMDMuOSA1MmMxMC4xIDUuMSAyMi4xIDUuMSAzMi4yIDBsMTAwLTUwYzEyLjItNi4xIDE5LjktMTguNiAxOS45LTMyLjJWMjgzLjljMC0xNS05LjMtMjguNC0yMy40LTMzLjd6TTM1OCAyMTQuOGwtODUgMzEuOXYtNjguMmw4NS0zN3Y3My4zek0xNTQgMTA0LjFsMTAyLTM4LjIgMTAyIDM4LjJ2LjZsLTEwMiA0MS40LTEwMi00MS40di0uNnptODQgMjkxLjFsLTg1IDQyLjV2LTc5LjFsODUtMzguOHY3NS40em0wLTExMmwtMTAyIDQxLjQtMTAyLTQxLjR2LS42bDEwMi0zOC4yIDEwMiAzOC4ydi42em0yNDAgMTEybC04NSA0Mi41di03OS4xbDg1LTM4Ljh2NzUuNHptMC0xMTJsLTEwMiA0MS40LTEwMi00MS40di0uNmwxMDItMzguMiAxMDIgMzguMnYuNnoiPjwvcGF0aD48L3N2Zz4K
//!
//! <br>
//!
//! <table><tr><td><hr>
//! <b>Note:</b> <i>As of Rust 1.45 this crate is superseded by native support
//! for #[proc_macro] in expression position. Only consider using this crate if
//! you care about supporting compilers between 1.31 and 1.45.</i>
//! <hr></td></tr></table>
//!
//! Since Rust 1.30, the language supports user-defined function-like procedural
//! macros. However these can only be invoked in item position, not in
//! statements or expressions.
//!
//! This crate implements an alternative type of procedural macro that can be
//! invoked in statement or expression position.
//!
//! # Defining procedural macros
//!
//! Two crates are required to define a procedural macro.
//!
//! ## The implementation crate
//!
//! This crate must contain nothing but procedural macros. Private helper
//! functions and private modules are fine but nothing can be public.
//!
//! [&raquo; example of an implementation crate][demo-hack-impl]
//!
//! Just like you would use a #\[proc_macro\] attribute to define a natively
//! supported procedural macro, use proc-macro-hack's #\[proc_macro_hack\]
//! attribute to define a procedural macro that works in expression position.
//! The function signature is the same as for ordinary function-like procedural
//! macros.
//!
//! ```
//! # extern crate proc_macro;
//! #
//! use proc_macro::TokenStream;
//! use proc_macro_hack::proc_macro_hack;
//! use quote::quote;
//! use syn::{parse_macro_input, Expr};
//!
//! # const IGNORE: &str = stringify! {
//! #[proc_macro_hack]
//! # };
//! pub fn add_one(input: TokenStream) -> TokenStream {
//!     let expr = parse_macro_input!(input as Expr);
//!     TokenStream::from(quote! {
//!         1 + (#expr)
//!     })
//! }
//! #
//! # fn main() {}
//! ```
//!
//! ## The declaration crate
//!
//! This crate is allowed to contain other public things if you need, for
//! example traits or functions or ordinary macros.
//!
//! [&raquo; example of a declaration crate][demo-hack]
//!
//! Within the declaration crate there needs to be a re-export of your
//! procedural macro from the implementation crate. The re-export also carries a
//! \#\[proc_macro_hack\] attribute.
//!
//! ```
//! use proc_macro_hack::proc_macro_hack;
//!
//! /// Add one to an expression.
//! ///
//! /// (Documentation goes here on the re-export, not in the other crate.)
//! #[proc_macro_hack]
//! pub use demo_hack_impl::add_one;
//! #
//! # fn main() {}
//! ```
//!
//! Both crates depend on `proc-macro-hack`:
//!
//! ```toml
//! [dependencies]
//! proc-macro-hack = "0.5"
//! ```
//!
//! Additionally, your implementation crate (but not your declaration crate) is
//! a proc macro crate:
//!
//! ```toml
//! [lib]
//! proc-macro = true
//! ```
//!
//! # Using procedural macros
//!
//! Users of your crate depend on your declaration crate (not your
//! implementation crate), then use your procedural macros as usual.
//!
//! [&raquo; example of a downstream crate][example]
//!
//! ```
//! use demo_hack::add_one;
//!
//! fn main() {
//!     let two = 2;
//!     let nine = add_one!(two) + add_one!(2 + 3);
//!     println!("nine = {}", nine);
//! }
//! ```
//!
//! [demo-hack-impl]: https://github.com/dtolnay/proc-macro-hack/tree/master/demo-hack-impl
//! [demo-hack]: https://github.com/dtolnay/proc-macro-hack/tree/master/demo-hack
//! [example]: https://github.com/dtolnay/proc-macro-hack/tree/master/example
//!
//! # Limitations
//!
//! - Only proc macros in expression position are supported. Proc macros in
//!   pattern position ([#20]) are not supported.
//!
//! - By default, nested invocations are not supported i.e. the code emitted by
//!   a proc-macro-hack macro invocation cannot contain recursive calls to the
//!   same proc-macro-hack macro nor calls to any other proc-macro-hack macros.
//!   Use [`proc-macro-nested`] if you require support for nested invocations.
//!
//! - By default, hygiene is structured such that the expanded code can't refer
//!   to local variables other than those passed by name somewhere in the macro
//!   input. If your macro must refer to *local* variables that don't get named
//!   in the macro input, use `#[proc_macro_hack(fake_call_site)]` on the
//!   re-export in your declaration crate. *Most macros won't need this.*
//!
//! - On compilers that are new enough to natively support proc macros in
//!   expression position, proc-macro-hack does not automatically use that
//!   support, since the hygiene can be subtly different between the two
//!   implementations. To opt in to compiling your macro to native
//!   `#[proc_macro]` on sufficiently new compilers, use
//!   `#[proc_macro_hack(only_hack_old_rustc)]` on the re-export in your
//!   declaration crate.
//!
//! [#10]: https://github.com/dtolnay/proc-macro-hack/issues/10
//! [#20]: https://github.com/dtolnay/proc-macro-hack/issues/20
//! [`proc-macro-nested`]: https://docs.rs/proc-macro-nested

#![recursion_limit = "512"]
#![allow(clippy::needless_doctest_main, clippy::toplevel_ref_arg)]

extern crate proc_macro;

#[macro_use]
mod quote;

mod error;
mod iter;
mod parse;

use crate::error::{compile_error, Error};
use crate::iter::Iter;
use crate::parse::{
    parse_define_args, parse_enum_hack, parse_export_args, parse_fake_call_site, parse_input,
};
use proc_macro::{Ident, Punct, Spacing, Span, TokenStream, TokenTree};
use std::fmt::Write;

type Visibility = Option<Ident>;

enum Input {
    Export(Export),
    Define(Define),
}

// pub use demo_hack_impl::{m1, m2 as qrst};
struct Export {
    attrs: TokenStream,
    vis: Visibility,
    from: Ident,
    macros: Vec<Macro>,
}

// pub fn m1(input: TokenStream) -> TokenStream { ... }
struct Define {
    attrs: TokenStream,
    name: Ident,
    body: TokenStream,
}

struct Macro {
    name: Ident,
    export_as: Ident,
}

#[proc_macro_attribute]
pub fn proc_macro_hack(args: TokenStream, input: TokenStream) -> TokenStream {
    let ref mut args = iter::new(args);
    let ref mut input = iter::new(input);
    expand_proc_macro_hack(args, input).unwrap_or_else(compile_error)
}

fn expand_proc_macro_hack(args: Iter, input: Iter) -> Result<TokenStream, Error> {
    match parse_input(input)? {
        Input::Export(export) => {
            let args = parse_export_args(args)?;
            Ok(expand_export(export, args))
        }
        Input::Define(define) => {
            parse_define_args(args)?;
            Ok(expand_define(define))
        }
    }
}

#[doc(hidden)]
#[proc_macro_derive(ProcMacroHack)]
pub fn enum_hack(input: TokenStream) -> TokenStream {
    let ref mut input = iter::new(input);
    parse_enum_hack(input).unwrap_or_else(compile_error)
}

struct FakeCallSite {
    derive: Ident,
    rest: TokenStream,
}

#[doc(hidden)]
#[proc_macro_attribute]
pub fn fake_call_site(args: TokenStream, input: TokenStream) -> TokenStream {
    let ref mut args = iter::new(args);
    let ref mut input = iter::new(input);
    expand_fake_call_site(args, input).unwrap_or_else(compile_error)
}

fn expand_fake_call_site(args: Iter, input: Iter) -> Result<TokenStream, Error> {
    let span = match args.next() {
        Some(token) => token.span(),
        None => return Ok(input.collect()),
    };

    let input = parse_fake_call_site(input)?;
    let mut derive = input.derive;
    derive.set_span(span);
    let rest = input.rest;

    Ok(quote! {
        #[derive(#derive)]
        #rest
    })
}

struct ExportArgs {
    support_nested: bool,
    internal_macro_calls: u16,
    fake_call_site: bool,
    only_hack_old_rustc: bool,
}

fn expand_export(export: Export, args: ExportArgs) -> TokenStream {
    if args.only_hack_old_rustc && cfg!(not(need_proc_macro_hack)) {
        return expand_export_nohack(export);
    }

    let dummy = dummy_name_for_export(&export);

    let attrs = export.attrs;
    let vis = export.vis;
    let macro_export = match vis {
        Some(_) => quote!(#[macro_export]),
        None => quote!(),
    };
    let crate_prefix = vis.as_ref().map(|_| quote!($crate::));
    let enum_variant = if args.support_nested {
        if args.internal_macro_calls == 0 {
            Ident::new("Nested", Span::call_site())
        } else {
            let name = format!("Nested{}", args.internal_macro_calls);
            Ident::new(&name, Span::call_site())
        }
    } else {
        Ident::new("Value", Span::call_site())
    };

    let from = export.from;
    let mut actual_names = TokenStream::new();
    let mut export_dispatch = TokenStream::new();
    let mut export_call_site = TokenStream::new();
    let mut macro_rules = TokenStream::new();
    for Macro { name, export_as } in &export.macros {
        let hacked = hacked_proc_macro_name(&name);
        let dispatch = dispatch_macro_name(&name);
        let call_site = call_site_macro_name(&name);

        if !actual_names.is_empty() {
            actual_names.extend(quote!(,));
        }
        actual_names.extend(quote!(#hacked));

        if !export_dispatch.is_empty() {
            export_dispatch.extend(quote!(,));
        }
        export_dispatch.extend(quote!(dispatch as #dispatch));

        if !export_call_site.is_empty() {
            export_call_site.extend(quote!(,));
        }
        export_call_site.extend(quote!(fake_call_site as #call_site));

        let do_derive = if !args.fake_call_site {
            quote! {
                #[derive(#crate_prefix #hacked)]
            }
        } else if crate_prefix.is_some() {
            quote! {
                use #crate_prefix #hacked;
                #[#crate_prefix #call_site ($($proc_macro)*)]
                #[derive(#hacked)]
            }
        } else {
            quote! {
                #[#call_site ($($proc_macro)*)]
                #[derive(#hacked)]
            }
        };

        let proc_macro_call = if args.support_nested {
            let extra_bangs = (0..args.internal_macro_calls)
                .map(|_| TokenTree::Punct(Punct::new('!', Spacing::Alone)))
                .collect::<TokenStream>();
            quote! {
                #crate_prefix #dispatch! { ($($proc_macro)*) #extra_bangs }
            }
        } else {
            quote! {
                proc_macro_call!()
            }
        };

        macro_rules.extend(quote! {
            #attrs
            #macro_export
            macro_rules! #export_as {
                ($($proc_macro:tt)*) => {{
                    #do_derive
                    #[allow(dead_code)]
                    enum ProcMacroHack {
                        #enum_variant = (stringify! { $($proc_macro)* }, 0).1,
                    }
                    #proc_macro_call
                }};
            }
        });
    }

    if export.macros.len() != 1 {
        export_dispatch = quote!({#export_dispatch});
        export_call_site = quote!({#export_call_site});
        actual_names = quote!({#actual_names});
    }

    let export_dispatch = if args.support_nested {
        quote! {
            #[doc(hidden)]
            #vis use proc_macro_nested::#export_dispatch;
        }
    } else {
        quote!()
    };

    let export_call_site = if args.fake_call_site {
        quote! {
            #[doc(hidden)]
            #vis use proc_macro_hack::#export_call_site;
        }
    } else {
        quote!()
    };

    let expanded = quote! {
        #[doc(hidden)]
        #vis use #from::#actual_names;

        #export_dispatch
        #export_call_site

        #macro_rules
    };

    wrap_in_enum_hack(dummy, expanded)
}

fn expand_export_nohack(export: Export) -> TokenStream {
    let attrs = export.attrs;
    let vis = export.vis;
    let from = export.from;
    let mut names = TokenStream::new();

    for Macro { name, export_as } in &export.macros {
        let pub_name = pub_proc_macro_name(&name);
        if !names.is_empty() {
            names.extend(quote!(,));
        }
        names.extend(quote!(#pub_name as #export_as));
    }

    if export.macros.len() != 1 {
        names = quote!({#names});
    }

    quote! {
        #attrs
        #vis use #from::#names;
    }
}

fn expand_define(define: Define) -> TokenStream {
    let attrs = define.attrs;
    let name = define.name;
    let pub_name = pub_proc_macro_name(&name);
    let hacked = hacked_proc_macro_name(&name);
    let body = define.body;

    quote! {
        mod #pub_name {
            extern crate proc_macro;
            pub use self::proc_macro::*;
        }

        #attrs
        #[doc(hidden)]
        #[proc_macro_derive(#hacked)]
        pub fn #hacked(input: #pub_name::TokenStream) -> #pub_name::TokenStream {
            use std::iter::FromIterator;

            let mut iter = input.into_iter();
            iter.next().unwrap(); // `enum`
            iter.next().unwrap(); // `ProcMacroHack`
            iter.next().unwrap(); // `#`
            iter.next().unwrap(); // `[allow(dead_code)]`

            let mut braces = match iter.next().unwrap() {
                #pub_name::TokenTree::Group(group) => group.stream().into_iter(),
                _ => unimplemented!(),
            };
            let variant = braces.next().unwrap(); // `Value` or `Nested`
            let varname = variant.to_string();
            let support_nested = varname.starts_with("Nested");
            braces.next().unwrap(); // `=`

            let mut parens = match braces.next().unwrap() {
                #pub_name::TokenTree::Group(group) => group.stream().into_iter(),
                _ => unimplemented!(),
            };
            parens.next().unwrap(); // `stringify`
            parens.next().unwrap(); // `!`

            let inner = match parens.next().unwrap() {
                #pub_name::TokenTree::Group(group) => group.stream(),
                _ => unimplemented!(),
            };

            let output: #pub_name::TokenStream = #name(inner.clone());

            fn count_bangs(input: #pub_name::TokenStream) -> usize {
                let mut count = 0;
                for token in input {
                    match token {
                        #pub_name::TokenTree::Punct(punct) => {
                            if punct.as_char() == '!' {
                                count += 1;
                            }
                        }
                        #pub_name::TokenTree::Group(group) => {
                            count += count_bangs(group.stream());
                        }
                        _ => {}
                    }
                }
                count
            }

            // macro_rules! proc_macro_call {
            //     () => { #output }
            // }
            #pub_name::TokenStream::from_iter(vec![
                #pub_name::TokenTree::Ident(
                    #pub_name::Ident::new("macro_rules", #pub_name::Span::call_site()),
                ),
                #pub_name::TokenTree::Punct(
                    #pub_name::Punct::new('!', #pub_name::Spacing::Alone),
                ),
                #pub_name::TokenTree::Ident(
                    #pub_name::Ident::new(
                        &if support_nested {
                            let extra_bangs = if varname == "Nested" {
                                0
                            } else {
                                varname["Nested".len()..].parse().unwrap()
                            };
                            format!("proc_macro_call_{}", extra_bangs + count_bangs(inner))
                        } else {
                            String::from("proc_macro_call")
                        },
                        #pub_name::Span::call_site(),
                    ),
                ),
                #pub_name::TokenTree::Group(
                    #pub_name::Group::new(#pub_name::Delimiter::Brace, #pub_name::TokenStream::from_iter(vec![
                        #pub_name::TokenTree::Group(
                            #pub_name::Group::new(#pub_name::Delimiter::Parenthesis, #pub_name::TokenStream::new()),
                        ),
                        #pub_name::TokenTree::Punct(
                            #pub_name::Punct::new('=', #pub_name::Spacing::Joint),
                        ),
                        #pub_name::TokenTree::Punct(
                            #pub_name::Punct::new('>', #pub_name::Spacing::Alone),
                        ),
                        #pub_name::TokenTree::Group(
                            #pub_name::Group::new(#pub_name::Delimiter::Brace, output),
                        ),
                    ])),
                ),
            ])
        }

        #attrs
        #[proc_macro]
        pub fn #pub_name(input: #pub_name::TokenStream) -> #pub_name::TokenStream {
            #name(input)
        }

        fn #name #body
    }
}

fn pub_proc_macro_name(conceptual: &Ident) -> Ident {
    Ident::new(
        &format!("proc_macro_hack_{}", conceptual),
        conceptual.span(),
    )
}

fn hacked_proc_macro_name(conceptual: &Ident) -> Ident {
    Ident::new(
        &format!("_proc_macro_hack_{}", conceptual),
        conceptual.span(),
    )
}

fn dispatch_macro_name(conceptual: &Ident) -> Ident {
    Ident::new(
        &format!("proc_macro_call_{}", conceptual),
        conceptual.span(),
    )
}

fn call_site_macro_name(conceptual: &Ident) -> Ident {
    Ident::new(
        &format!("proc_macro_fake_call_site_{}", conceptual),
        conceptual.span(),
    )
}

fn dummy_name_for_export(export: &Export) -> String {
    let mut dummy = String::new();
    let from = unraw(&export.from).to_string();
    write!(dummy, "_{}{}", from.len(), from).unwrap();
    for m in &export.macros {
        let name = unraw(&m.name).to_string();
        write!(dummy, "_{}{}", name.len(), name).unwrap();
    }
    dummy
}

fn unraw(ident: &Ident) -> Ident {
    let string = ident.to_string();
    if string.starts_with("r#") {
        Ident::new(&string[2..], ident.span())
    } else {
        ident.clone()
    }
}

fn wrap_in_enum_hack(dummy: String, inner: TokenStream) -> TokenStream {
    let dummy = Ident::new(&dummy, Span::call_site());
    quote! {
        #[derive(proc_macro_hack::ProcMacroHack)]
        enum #dummy {
            Value = (stringify! { #inner }, 0).1,
        }
    }
}
