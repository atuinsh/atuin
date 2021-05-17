use proc_macro::{Delimiter, Group, Ident, Literal, Punct, Spacing, Span, TokenStream, TokenTree};
use std::iter::FromIterator;

pub struct Error {
    span: Span,
    msg: String,
}

impl Error {
    pub fn new(span: Span, msg: impl Into<String>) -> Self {
        Error {
            span,
            msg: msg.into(),
        }
    }
}

pub fn compile_error(err: Error) -> TokenStream {
    // compile_error!($msg)
    TokenStream::from_iter(vec![
        TokenTree::Ident(Ident::new("compile_error", err.span)),
        TokenTree::Punct({
            let mut punct = Punct::new('!', Spacing::Alone);
            punct.set_span(err.span);
            punct
        }),
        TokenTree::Group({
            let mut group = Group::new(Delimiter::Brace, {
                TokenStream::from_iter(vec![TokenTree::Literal({
                    let mut string = Literal::string(&err.msg);
                    string.set_span(err.span);
                    string
                })])
            });
            group.set_span(err.span);
            group
        }),
    ])
}
