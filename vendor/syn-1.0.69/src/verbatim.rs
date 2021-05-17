use crate::parse::{ParseBuffer, ParseStream};
use proc_macro2::TokenStream;
use std::iter;

pub fn between<'a>(begin: ParseBuffer<'a>, end: ParseStream<'a>) -> TokenStream {
    let end = end.cursor();
    let mut cursor = begin.cursor();
    let mut tokens = TokenStream::new();
    while cursor != end {
        let (tt, next) = cursor.token_tree().unwrap();
        tokens.extend(iter::once(tt));
        cursor = next;
    }
    tokens
}
