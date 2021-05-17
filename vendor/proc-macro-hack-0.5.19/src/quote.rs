use proc_macro::{Ident, TokenStream, TokenTree};
use std::iter;

macro_rules! quote {
    () => {
        ::proc_macro::TokenStream::new()
    };
    ($($tt:tt)*) => {{
        let mut tokens = ::proc_macro::TokenStream::new();
        quote_each_token!(tokens $($tt)*);
        tokens
    }};
}

macro_rules! quote_each_token {
    ($tokens:ident # $var:ident $($rest:tt)*) => {
        $crate::quote::Tokens::extend(&mut $tokens, &$var);
        quote_each_token!($tokens $($rest)*);
    };
    ($tokens:ident $ident:ident $($rest:tt)*) => {
        <::proc_macro::TokenStream as ::std::iter::Extend<_>>::extend(
            &mut $tokens,
            ::std::iter::once(
                ::proc_macro::TokenTree::Ident(
                    ::proc_macro::Ident::new(
                        stringify!($ident),
                        ::proc_macro::Span::call_site(),
                    ),
                ),
            ),
        );
        quote_each_token!($tokens $($rest)*);
    };
    ($tokens:ident ( $($inner:tt)* ) $($rest:tt)*) => {
        <::proc_macro::TokenStream as ::std::iter::Extend<_>>::extend(
            &mut $tokens,
            ::std::iter::once(
                ::proc_macro::TokenTree::Group(
                    ::proc_macro::Group::new(
                        ::proc_macro::Delimiter::Parenthesis,
                        quote!($($inner)*),
                    ),
                ),
            ),
        );
        quote_each_token!($tokens $($rest)*);
    };
    ($tokens:ident [ $($inner:tt)* ] $($rest:tt)*) => {
        <::proc_macro::TokenStream as ::std::iter::Extend<_>>::extend(
            &mut $tokens,
            ::std::iter::once(
                ::proc_macro::TokenTree::Group(
                    ::proc_macro::Group::new(
                        ::proc_macro::Delimiter::Bracket,
                        quote!($($inner)*),
                    ),
                ),
            ),
        );
        quote_each_token!($tokens $($rest)*);
    };
    ($tokens:ident { $($inner:tt)* } $($rest:tt)*) => {
        <::proc_macro::TokenStream as ::std::iter::Extend<_>>::extend(
            &mut $tokens,
            ::std::iter::once(
                ::proc_macro::TokenTree::Group(
                    ::proc_macro::Group::new(
                        ::proc_macro::Delimiter::Brace,
                        quote!($($inner)*),
                    ),
                ),
            ),
        );
        quote_each_token!($tokens $($rest)*);
    };
    ($tokens:ident $punct:tt $($rest:tt)*) => {
        <::proc_macro::TokenStream as ::std::iter::Extend<_>>::extend(
            &mut $tokens,
            stringify!($punct).parse::<::proc_macro::TokenStream>(),
        );
        quote_each_token!($tokens $($rest)*);
    };
    ($tokens:ident) => {};
}

pub trait Tokens {
    fn extend(tokens: &mut TokenStream, var: &Self);
}

impl Tokens for Ident {
    fn extend(tokens: &mut TokenStream, var: &Self) {
        tokens.extend(iter::once(TokenTree::Ident(var.clone())));
    }
}

impl Tokens for TokenStream {
    fn extend(tokens: &mut TokenStream, var: &Self) {
        tokens.extend(var.clone());
    }
}

impl<T: Tokens> Tokens for Option<T> {
    fn extend(tokens: &mut TokenStream, var: &Self) {
        if let Some(var) = var {
            T::extend(tokens, var);
        }
    }
}

impl<T: Tokens> Tokens for &T {
    fn extend(tokens: &mut TokenStream, var: &Self) {
        T::extend(tokens, var);
    }
}
