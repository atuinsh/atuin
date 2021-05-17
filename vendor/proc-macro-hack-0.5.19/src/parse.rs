use crate::iter::{self, Iter, IterImpl};
use crate::{Define, Error, Export, ExportArgs, FakeCallSite, Input, Macro, Visibility};
use proc_macro::Delimiter::{Brace, Bracket, Parenthesis};
use proc_macro::{Delimiter, Ident, Span, TokenStream, TokenTree};

pub(crate) fn parse_input(tokens: Iter) -> Result<Input, Error> {
    let attrs = parse_attributes(tokens)?;
    let vis = parse_visibility(tokens)?;
    let kw = parse_ident(tokens)?;
    if kw.to_string() == "use" {
        parse_export(attrs, vis, tokens).map(Input::Export)
    } else if kw.to_string() == "fn" {
        parse_define(attrs, vis, kw.span(), tokens).map(Input::Define)
    } else {
        Err(Error::new(
            kw.span(),
            "unexpected input to #[proc_macro_hack]",
        ))
    }
}

fn parse_export(attrs: TokenStream, vis: Visibility, tokens: Iter) -> Result<Export, Error> {
    let _ = parse_punct(tokens, ':');
    let _ = parse_punct(tokens, ':');
    let from = parse_ident(tokens)?;
    parse_punct(tokens, ':')?;
    parse_punct(tokens, ':')?;

    let mut macros = Vec::new();
    match tokens.peek() {
        Some(TokenTree::Group(group)) if group.delimiter() == Brace => {
            let ref mut content = iter::new(group.stream());
            loop {
                macros.push(parse_macro(content)?);
                if content.peek().is_none() {
                    break;
                }
                parse_punct(content, ',')?;
                if content.peek().is_none() {
                    break;
                }
            }
            tokens.next().unwrap();
        }
        _ => macros.push(parse_macro(tokens)?),
    }

    parse_punct(tokens, ';')?;
    Ok(Export {
        attrs,
        vis,
        from,
        macros,
    })
}

fn parse_punct(tokens: Iter, ch: char) -> Result<(), Error> {
    match tokens.peek() {
        Some(TokenTree::Punct(punct)) if punct.as_char() == ch => {
            tokens.next().unwrap();
            Ok(())
        }
        tt => Err(Error::new(
            tt.map_or_else(Span::call_site, TokenTree::span),
            format!("expected `{}`", ch),
        )),
    }
}

fn parse_define(
    attrs: TokenStream,
    vis: Visibility,
    fn_token: Span,
    tokens: Iter,
) -> Result<Define, Error> {
    if vis.is_none() {
        return Err(Error::new(
            fn_token,
            "functions tagged with `#[proc_macro_hack]` must be `pub`",
        ));
    }
    let name = parse_ident(tokens)?;
    let body = tokens.collect();
    Ok(Define { attrs, name, body })
}

fn parse_macro(tokens: Iter) -> Result<Macro, Error> {
    let name = parse_ident(tokens)?;
    let export_as = match tokens.peek() {
        Some(TokenTree::Ident(ident)) if ident.to_string() == "as" => {
            tokens.next().unwrap();
            parse_ident(tokens)?
        }
        _ => name.clone(),
    };
    Ok(Macro { name, export_as })
}

fn parse_ident(tokens: Iter) -> Result<Ident, Error> {
    match tokens.next() {
        Some(TokenTree::Ident(ident)) => Ok(ident),
        tt => Err(Error::new(
            tt.as_ref().map_or_else(Span::call_site, TokenTree::span),
            "expected identifier",
        )),
    }
}

fn parse_keyword(tokens: Iter, kw: &'static str) -> Result<(), Error> {
    match &tokens.next() {
        Some(TokenTree::Ident(ident)) if ident.to_string() == kw => Ok(()),
        tt => Err(Error::new(
            tt.as_ref().map_or_else(Span::call_site, TokenTree::span),
            format!("expected `{}`", kw),
        )),
    }
}

fn parse_int(tokens: Iter) -> Result<u16, Span> {
    match tokens.next() {
        Some(TokenTree::Literal(lit)) => lit.to_string().parse().map_err(|_| lit.span()),
        Some(tt) => Err(tt.span()),
        None => Err(Span::call_site()),
    }
}

fn parse_group(tokens: Iter, delimiter: Delimiter) -> Result<IterImpl, Error> {
    match &tokens.next() {
        Some(TokenTree::Group(group)) if group.delimiter() == delimiter => {
            Ok(iter::new(group.stream()))
        }
        tt => Err(Error::new(
            tt.as_ref().map_or_else(Span::call_site, TokenTree::span),
            "expected delimiter",
        )),
    }
}

fn parse_visibility(tokens: Iter) -> Result<Visibility, Error> {
    if let Some(TokenTree::Ident(ident)) = tokens.peek() {
        if ident.to_string() == "pub" {
            match tokens.next().unwrap() {
                TokenTree::Ident(vis) => return Ok(Some(vis)),
                _ => unreachable!(),
            }
        }
    }
    Ok(None)
}

fn parse_attributes(tokens: Iter) -> Result<TokenStream, Error> {
    let mut attrs = TokenStream::new();
    while let Some(TokenTree::Punct(punct)) = tokens.peek() {
        if punct.as_char() != '#' {
            break;
        }
        let span = punct.span();
        attrs.extend(tokens.next());
        match tokens.peek() {
            Some(TokenTree::Group(group)) if group.delimiter() == Bracket => {
                attrs.extend(tokens.next());
            }
            _ => return Err(Error::new(span, "unexpected input")),
        }
    }
    Ok(attrs)
}

pub(crate) fn parse_export_args(tokens: Iter) -> Result<ExportArgs, Error> {
    let mut args = ExportArgs {
        support_nested: false,
        internal_macro_calls: 0,
        fake_call_site: false,
        only_hack_old_rustc: false,
    };

    while let Some(tt) = tokens.next() {
        match &tt {
            TokenTree::Ident(ident) if ident.to_string() == "support_nested" => {
                args.support_nested = true;
            }
            TokenTree::Ident(ident) if ident.to_string() == "internal_macro_calls" => {
                parse_punct(tokens, '=')?;
                let calls = parse_int(tokens).map_err(|span| {
                    Error::new(span, "expected integer value for internal_macro_calls")
                })?;
                args.internal_macro_calls = calls;
            }
            TokenTree::Ident(ident) if ident.to_string() == "fake_call_site" => {
                args.fake_call_site = true;
            }
            TokenTree::Ident(ident) if ident.to_string() == "only_hack_old_rustc" => {
                args.only_hack_old_rustc = true;
            }
            _ => {
                return Err(Error::new(
                    tt.span(),
                    "expected one of: `support_nested`, `internal_macro_calls`, `fake_call_site`, `only_hack_old_rustc`",
                ));
            }
        }
        if tokens.peek().is_none() {
            break;
        }
        parse_punct(tokens, ',')?;
    }

    Ok(args)
}

pub(crate) fn parse_define_args(tokens: Iter) -> Result<(), Error> {
    match tokens.peek() {
        None => Ok(()),
        Some(token) => Err(Error::new(
            token.span(),
            "unexpected argument to proc_macro_hack macro implementation; args are only accepted on the macro declaration (the `pub use`)",
        )),
    }
}

pub(crate) fn parse_enum_hack(tokens: Iter) -> Result<TokenStream, Error> {
    parse_keyword(tokens, "enum")?;
    parse_ident(tokens)?;

    let ref mut braces = parse_group(tokens, Brace)?;
    parse_ident(braces)?;
    parse_punct(braces, '=')?;

    let ref mut parens = parse_group(braces, Parenthesis)?;
    parse_ident(parens)?;
    parse_punct(parens, '!')?;

    let ref mut inner = parse_group(parens, Brace)?;
    let token_stream = inner.collect();

    parse_punct(parens, ',')?;
    let _ = parens.next();
    parse_punct(braces, '.')?;
    let _ = braces.next();
    parse_punct(braces, ',')?;

    Ok(token_stream)
}

pub(crate) fn parse_fake_call_site(tokens: Iter) -> Result<FakeCallSite, Error> {
    parse_punct(tokens, '#')?;
    let ref mut attr = parse_group(tokens, Bracket)?;
    parse_keyword(attr, "derive")?;
    let ref mut path = parse_group(attr, Parenthesis)?;
    Ok(FakeCallSite {
        derive: parse_ident(path)?,
        rest: tokens.collect(),
    })
}
