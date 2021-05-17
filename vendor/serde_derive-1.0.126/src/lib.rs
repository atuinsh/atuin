//! This crate provides Serde's two derive macros.
//!
//! ```edition2018
//! # use serde_derive::{Serialize, Deserialize};
//! #
//! #[derive(Serialize, Deserialize)]
//! # struct S;
//! #
//! # fn main() {}
//! ```
//!
//! Please refer to [https://serde.rs/derive.html] for how to set this up.
//!
//! [https://serde.rs/derive.html]: https://serde.rs/derive.html

#![doc(html_root_url = "https://docs.rs/serde_derive/1.0.126")]
#![allow(unknown_lints, bare_trait_objects)]
#![deny(clippy::all, clippy::pedantic)]
// Ignored clippy lints
#![allow(
    // clippy false positive: https://github.com/rust-lang/rust-clippy/issues/7054
    clippy::branches_sharing_code,
    clippy::cognitive_complexity,
    clippy::enum_variant_names,
    // clippy bug: https://github.com/rust-lang/rust-clippy/issues/6797
    clippy::manual_map,
    clippy::match_like_matches_macro,
    clippy::needless_pass_by_value,
    clippy::too_many_arguments,
    clippy::trivially_copy_pass_by_ref,
    clippy::used_underscore_binding,
    clippy::wildcard_in_or_patterns,
    // clippy bug: https://github.com/rust-lang/rust-clippy/issues/5704
    clippy::unnested_or_patterns,
)]
// Ignored clippy_pedantic lints
#![allow(
    clippy::cast_possible_truncation,
    clippy::checked_conversions,
    clippy::doc_markdown,
    clippy::enum_glob_use,
    clippy::indexing_slicing,
    clippy::items_after_statements,
    clippy::let_underscore_drop,
    clippy::map_err_ignore,
    clippy::match_same_arms,
    // clippy bug: https://github.com/rust-lang/rust-clippy/issues/6984
    clippy::match_wildcard_for_single_variants,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::option_if_let_else,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::struct_excessive_bools,
    clippy::too_many_lines,
    clippy::unseparated_literal_suffix,
    clippy::unused_self,
    clippy::use_self,
    clippy::wildcard_imports
)]

#[macro_use]
extern crate quote;
#[macro_use]
extern crate syn;

extern crate proc_macro;
extern crate proc_macro2;

mod internals;

use proc_macro::TokenStream;
use syn::DeriveInput;

#[macro_use]
mod bound;
#[macro_use]
mod fragment;

mod de;
mod dummy;
mod pretend;
mod ser;
mod try;

#[proc_macro_derive(Serialize, attributes(serde))]
pub fn derive_serialize(input: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(input as DeriveInput);
    ser::expand_derive_serialize(&mut input)
        .unwrap_or_else(to_compile_errors)
        .into()
}

#[proc_macro_derive(Deserialize, attributes(serde))]
pub fn derive_deserialize(input: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(input as DeriveInput);
    de::expand_derive_deserialize(&mut input)
        .unwrap_or_else(to_compile_errors)
        .into()
}

fn to_compile_errors(errors: Vec<syn::Error>) -> proc_macro2::TokenStream {
    let compile_errors = errors.iter().map(syn::Error::to_compile_error);
    quote!(#(#compile_errors)*)
}
