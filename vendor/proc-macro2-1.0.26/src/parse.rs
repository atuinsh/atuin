use crate::fallback::{
    is_ident_continue, is_ident_start, Group, LexError, Literal, Span, TokenStream,
};
use crate::{Delimiter, Punct, Spacing, TokenTree};
use std::char;
use std::str::{Bytes, CharIndices, Chars};

#[derive(Copy, Clone, Eq, PartialEq)]
pub(crate) struct Cursor<'a> {
    pub rest: &'a str,
    #[cfg(span_locations)]
    pub off: u32,
}

impl<'a> Cursor<'a> {
    fn advance(&self, bytes: usize) -> Cursor<'a> {
        let (_front, rest) = self.rest.split_at(bytes);
        Cursor {
            rest,
            #[cfg(span_locations)]
            off: self.off + _front.chars().count() as u32,
        }
    }

    fn starts_with(&self, s: &str) -> bool {
        self.rest.starts_with(s)
    }

    fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }

    fn len(&self) -> usize {
        self.rest.len()
    }

    fn as_bytes(&self) -> &'a [u8] {
        self.rest.as_bytes()
    }

    fn bytes(&self) -> Bytes<'a> {
        self.rest.bytes()
    }

    fn chars(&self) -> Chars<'a> {
        self.rest.chars()
    }

    fn char_indices(&self) -> CharIndices<'a> {
        self.rest.char_indices()
    }

    fn parse(&self, tag: &str) -> Result<Cursor<'a>, Reject> {
        if self.starts_with(tag) {
            Ok(self.advance(tag.len()))
        } else {
            Err(Reject)
        }
    }
}

struct Reject;
type PResult<'a, O> = Result<(Cursor<'a>, O), Reject>;

fn skip_whitespace(input: Cursor) -> Cursor {
    let mut s = input;

    while !s.is_empty() {
        let byte = s.as_bytes()[0];
        if byte == b'/' {
            if s.starts_with("//")
                && (!s.starts_with("///") || s.starts_with("////"))
                && !s.starts_with("//!")
            {
                let (cursor, _) = take_until_newline_or_eof(s);
                s = cursor;
                continue;
            } else if s.starts_with("/**/") {
                s = s.advance(4);
                continue;
            } else if s.starts_with("/*")
                && (!s.starts_with("/**") || s.starts_with("/***"))
                && !s.starts_with("/*!")
            {
                match block_comment(s) {
                    Ok((rest, _)) => {
                        s = rest;
                        continue;
                    }
                    Err(Reject) => return s,
                }
            }
        }
        match byte {
            b' ' | 0x09..=0x0d => {
                s = s.advance(1);
                continue;
            }
            b if b <= 0x7f => {}
            _ => {
                let ch = s.chars().next().unwrap();
                if is_whitespace(ch) {
                    s = s.advance(ch.len_utf8());
                    continue;
                }
            }
        }
        return s;
    }
    s
}

fn block_comment(input: Cursor) -> PResult<&str> {
    if !input.starts_with("/*") {
        return Err(Reject);
    }

    let mut depth = 0;
    let bytes = input.as_bytes();
    let mut i = 0;
    let upper = bytes.len() - 1;

    while i < upper {
        if bytes[i] == b'/' && bytes[i + 1] == b'*' {
            depth += 1;
            i += 1; // eat '*'
        } else if bytes[i] == b'*' && bytes[i + 1] == b'/' {
            depth -= 1;
            if depth == 0 {
                return Ok((input.advance(i + 2), &input.rest[..i + 2]));
            }
            i += 1; // eat '/'
        }
        i += 1;
    }

    Err(Reject)
}

fn is_whitespace(ch: char) -> bool {
    // Rust treats left-to-right mark and right-to-left mark as whitespace
    ch.is_whitespace() || ch == '\u{200e}' || ch == '\u{200f}'
}

fn word_break(input: Cursor) -> Result<Cursor, Reject> {
    match input.chars().next() {
        Some(ch) if is_ident_continue(ch) => Err(Reject),
        Some(_) | None => Ok(input),
    }
}

pub(crate) fn token_stream(mut input: Cursor) -> Result<TokenStream, LexError> {
    let mut trees = Vec::new();
    let mut stack = Vec::new();

    loop {
        input = skip_whitespace(input);

        if let Ok((rest, tt)) = doc_comment(input) {
            trees.extend(tt);
            input = rest;
            continue;
        }

        #[cfg(span_locations)]
        let lo = input.off;

        let first = match input.bytes().next() {
            Some(first) => first,
            None => match stack.last() {
                None => return Ok(TokenStream { inner: trees }),
                #[cfg(span_locations)]
                Some((lo, _frame)) => {
                    return Err(LexError {
                        span: Span { lo: *lo, hi: *lo },
                    })
                }
                #[cfg(not(span_locations))]
                Some(_frame) => return Err(LexError { span: Span {} }),
            },
        };

        if let Some(open_delimiter) = match first {
            b'(' => Some(Delimiter::Parenthesis),
            b'[' => Some(Delimiter::Bracket),
            b'{' => Some(Delimiter::Brace),
            _ => None,
        } {
            input = input.advance(1);
            let frame = (open_delimiter, trees);
            #[cfg(span_locations)]
            let frame = (lo, frame);
            stack.push(frame);
            trees = Vec::new();
        } else if let Some(close_delimiter) = match first {
            b')' => Some(Delimiter::Parenthesis),
            b']' => Some(Delimiter::Bracket),
            b'}' => Some(Delimiter::Brace),
            _ => None,
        } {
            let frame = match stack.pop() {
                Some(frame) => frame,
                None => return Err(lex_error(input)),
            };
            #[cfg(span_locations)]
            let (lo, frame) = frame;
            let (open_delimiter, outer) = frame;
            if open_delimiter != close_delimiter {
                return Err(lex_error(input));
            }
            input = input.advance(1);
            let mut g = Group::new(open_delimiter, TokenStream { inner: trees });
            g.set_span(Span {
                #[cfg(span_locations)]
                lo,
                #[cfg(span_locations)]
                hi: input.off,
            });
            trees = outer;
            trees.push(TokenTree::Group(crate::Group::_new_stable(g)));
        } else {
            let (rest, mut tt) = match leaf_token(input) {
                Ok((rest, tt)) => (rest, tt),
                Err(Reject) => return Err(lex_error(input)),
            };
            tt.set_span(crate::Span::_new_stable(Span {
                #[cfg(span_locations)]
                lo,
                #[cfg(span_locations)]
                hi: rest.off,
            }));
            trees.push(tt);
            input = rest;
        }
    }
}

fn lex_error(cursor: Cursor) -> LexError {
    #[cfg(not(span_locations))]
    let _ = cursor;
    LexError {
        span: Span {
            #[cfg(span_locations)]
            lo: cursor.off,
            #[cfg(span_locations)]
            hi: cursor.off,
        },
    }
}

fn leaf_token(input: Cursor) -> PResult<TokenTree> {
    if let Ok((input, l)) = literal(input) {
        // must be parsed before ident
        Ok((input, TokenTree::Literal(crate::Literal::_new_stable(l))))
    } else if let Ok((input, p)) = punct(input) {
        Ok((input, TokenTree::Punct(p)))
    } else if let Ok((input, i)) = ident(input) {
        Ok((input, TokenTree::Ident(i)))
    } else {
        Err(Reject)
    }
}

fn ident(input: Cursor) -> PResult<crate::Ident> {
    if ["r\"", "r#\"", "r##", "b\"", "b\'", "br\"", "br#"]
        .iter()
        .any(|prefix| input.starts_with(prefix))
    {
        Err(Reject)
    } else {
        ident_any(input)
    }
}

fn ident_any(input: Cursor) -> PResult<crate::Ident> {
    let raw = input.starts_with("r#");
    let rest = input.advance((raw as usize) << 1);

    let (rest, sym) = ident_not_raw(rest)?;

    if !raw {
        let ident = crate::Ident::new(sym, crate::Span::call_site());
        return Ok((rest, ident));
    }

    if sym == "_" {
        return Err(Reject);
    }

    let ident = crate::Ident::_new_raw(sym, crate::Span::call_site());
    Ok((rest, ident))
}

fn ident_not_raw(input: Cursor) -> PResult<&str> {
    let mut chars = input.char_indices();

    match chars.next() {
        Some((_, ch)) if is_ident_start(ch) => {}
        _ => return Err(Reject),
    }

    let mut end = input.len();
    for (i, ch) in chars {
        if !is_ident_continue(ch) {
            end = i;
            break;
        }
    }

    Ok((input.advance(end), &input.rest[..end]))
}

fn literal(input: Cursor) -> PResult<Literal> {
    let rest = literal_nocapture(input)?;
    let end = input.len() - rest.len();
    Ok((rest, Literal::_new(input.rest[..end].to_string())))
}

fn literal_nocapture(input: Cursor) -> Result<Cursor, Reject> {
    if let Ok(ok) = string(input) {
        Ok(ok)
    } else if let Ok(ok) = byte_string(input) {
        Ok(ok)
    } else if let Ok(ok) = byte(input) {
        Ok(ok)
    } else if let Ok(ok) = character(input) {
        Ok(ok)
    } else if let Ok(ok) = float(input) {
        Ok(ok)
    } else if let Ok(ok) = int(input) {
        Ok(ok)
    } else {
        Err(Reject)
    }
}

fn literal_suffix(input: Cursor) -> Cursor {
    match ident_not_raw(input) {
        Ok((input, _)) => input,
        Err(Reject) => input,
    }
}

fn string(input: Cursor) -> Result<Cursor, Reject> {
    if let Ok(input) = input.parse("\"") {
        cooked_string(input)
    } else if let Ok(input) = input.parse("r") {
        raw_string(input)
    } else {
        Err(Reject)
    }
}

fn cooked_string(input: Cursor) -> Result<Cursor, Reject> {
    let mut chars = input.char_indices().peekable();

    while let Some((i, ch)) = chars.next() {
        match ch {
            '"' => {
                let input = input.advance(i + 1);
                return Ok(literal_suffix(input));
            }
            '\r' => match chars.next() {
                Some((_, '\n')) => {}
                _ => break,
            },
            '\\' => match chars.next() {
                Some((_, 'x')) => {
                    if !backslash_x_char(&mut chars) {
                        break;
                    }
                }
                Some((_, 'n')) | Some((_, 'r')) | Some((_, 't')) | Some((_, '\\'))
                | Some((_, '\'')) | Some((_, '"')) | Some((_, '0')) => {}
                Some((_, 'u')) => {
                    if !backslash_u(&mut chars) {
                        break;
                    }
                }
                Some((_, ch @ '\n')) | Some((_, ch @ '\r')) => {
                    let mut last = ch;
                    loop {
                        if last == '\r' && chars.next().map_or(true, |(_, ch)| ch != '\n') {
                            return Err(Reject);
                        }
                        match chars.peek() {
                            Some((_, ch)) if ch.is_whitespace() => {
                                last = *ch;
                                chars.next();
                            }
                            _ => break,
                        }
                    }
                }
                _ => break,
            },
            _ch => {}
        }
    }
    Err(Reject)
}

fn byte_string(input: Cursor) -> Result<Cursor, Reject> {
    if let Ok(input) = input.parse("b\"") {
        cooked_byte_string(input)
    } else if let Ok(input) = input.parse("br") {
        raw_string(input)
    } else {
        Err(Reject)
    }
}

fn cooked_byte_string(mut input: Cursor) -> Result<Cursor, Reject> {
    let mut bytes = input.bytes().enumerate();
    while let Some((offset, b)) = bytes.next() {
        match b {
            b'"' => {
                let input = input.advance(offset + 1);
                return Ok(literal_suffix(input));
            }
            b'\r' => match bytes.next() {
                Some((_, b'\n')) => {}
                _ => break,
            },
            b'\\' => match bytes.next() {
                Some((_, b'x')) => {
                    if !backslash_x_byte(&mut bytes) {
                        break;
                    }
                }
                Some((_, b'n')) | Some((_, b'r')) | Some((_, b't')) | Some((_, b'\\'))
                | Some((_, b'0')) | Some((_, b'\'')) | Some((_, b'"')) => {}
                Some((newline, b @ b'\n')) | Some((newline, b @ b'\r')) => {
                    let mut last = b as char;
                    let rest = input.advance(newline + 1);
                    let mut chars = rest.char_indices();
                    loop {
                        if last == '\r' && chars.next().map_or(true, |(_, ch)| ch != '\n') {
                            return Err(Reject);
                        }
                        match chars.next() {
                            Some((_, ch)) if ch.is_whitespace() => last = ch,
                            Some((offset, _)) => {
                                input = rest.advance(offset);
                                bytes = input.bytes().enumerate();
                                break;
                            }
                            None => return Err(Reject),
                        }
                    }
                }
                _ => break,
            },
            b if b < 0x80 => {}
            _ => break,
        }
    }
    Err(Reject)
}

fn raw_string(input: Cursor) -> Result<Cursor, Reject> {
    let mut chars = input.char_indices();
    let mut n = 0;
    while let Some((i, ch)) = chars.next() {
        match ch {
            '"' => {
                n = i;
                break;
            }
            '#' => {}
            _ => return Err(Reject),
        }
    }
    while let Some((i, ch)) = chars.next() {
        match ch {
            '"' if input.rest[i + 1..].starts_with(&input.rest[..n]) => {
                let rest = input.advance(i + 1 + n);
                return Ok(literal_suffix(rest));
            }
            '\r' => match chars.next() {
                Some((_, '\n')) => {}
                _ => break,
            },
            _ => {}
        }
    }
    Err(Reject)
}

fn byte(input: Cursor) -> Result<Cursor, Reject> {
    let input = input.parse("b'")?;
    let mut bytes = input.bytes().enumerate();
    let ok = match bytes.next().map(|(_, b)| b) {
        Some(b'\\') => match bytes.next().map(|(_, b)| b) {
            Some(b'x') => backslash_x_byte(&mut bytes),
            Some(b'n') | Some(b'r') | Some(b't') | Some(b'\\') | Some(b'0') | Some(b'\'')
            | Some(b'"') => true,
            _ => false,
        },
        b => b.is_some(),
    };
    if !ok {
        return Err(Reject);
    }
    let (offset, _) = bytes.next().ok_or(Reject)?;
    if !input.chars().as_str().is_char_boundary(offset) {
        return Err(Reject);
    }
    let input = input.advance(offset).parse("'")?;
    Ok(literal_suffix(input))
}

fn character(input: Cursor) -> Result<Cursor, Reject> {
    let input = input.parse("'")?;
    let mut chars = input.char_indices();
    let ok = match chars.next().map(|(_, ch)| ch) {
        Some('\\') => match chars.next().map(|(_, ch)| ch) {
            Some('x') => backslash_x_char(&mut chars),
            Some('u') => backslash_u(&mut chars),
            Some('n') | Some('r') | Some('t') | Some('\\') | Some('0') | Some('\'') | Some('"') => {
                true
            }
            _ => false,
        },
        ch => ch.is_some(),
    };
    if !ok {
        return Err(Reject);
    }
    let (idx, _) = chars.next().ok_or(Reject)?;
    let input = input.advance(idx).parse("'")?;
    Ok(literal_suffix(input))
}

macro_rules! next_ch {
    ($chars:ident @ $pat:pat $(| $rest:pat)*) => {
        match $chars.next() {
            Some((_, ch)) => match ch {
                $pat $(| $rest)* => ch,
                _ => return false,
            },
            None => return false,
        }
    };
}

fn backslash_x_char<I>(chars: &mut I) -> bool
where
    I: Iterator<Item = (usize, char)>,
{
    next_ch!(chars @ '0'..='7');
    next_ch!(chars @ '0'..='9' | 'a'..='f' | 'A'..='F');
    true
}

fn backslash_x_byte<I>(chars: &mut I) -> bool
where
    I: Iterator<Item = (usize, u8)>,
{
    next_ch!(chars @ b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F');
    next_ch!(chars @ b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F');
    true
}

fn backslash_u<I>(chars: &mut I) -> bool
where
    I: Iterator<Item = (usize, char)>,
{
    next_ch!(chars @ '{');
    let mut value = 0;
    let mut len = 0;
    for (_, ch) in chars {
        let digit = match ch {
            '0'..='9' => ch as u8 - b'0',
            'a'..='f' => 10 + ch as u8 - b'a',
            'A'..='F' => 10 + ch as u8 - b'A',
            '_' if len > 0 => continue,
            '}' if len > 0 => return char::from_u32(value).is_some(),
            _ => return false,
        };
        if len == 6 {
            return false;
        }
        value *= 0x10;
        value += u32::from(digit);
        len += 1;
    }
    false
}

fn float(input: Cursor) -> Result<Cursor, Reject> {
    let mut rest = float_digits(input)?;
    if let Some(ch) = rest.chars().next() {
        if is_ident_start(ch) {
            rest = ident_not_raw(rest)?.0;
        }
    }
    word_break(rest)
}

fn float_digits(input: Cursor) -> Result<Cursor, Reject> {
    let mut chars = input.chars().peekable();
    match chars.next() {
        Some(ch) if ch >= '0' && ch <= '9' => {}
        _ => return Err(Reject),
    }

    let mut len = 1;
    let mut has_dot = false;
    let mut has_exp = false;
    while let Some(&ch) = chars.peek() {
        match ch {
            '0'..='9' | '_' => {
                chars.next();
                len += 1;
            }
            '.' => {
                if has_dot {
                    break;
                }
                chars.next();
                if chars
                    .peek()
                    .map(|&ch| ch == '.' || is_ident_start(ch))
                    .unwrap_or(false)
                {
                    return Err(Reject);
                }
                len += 1;
                has_dot = true;
            }
            'e' | 'E' => {
                chars.next();
                len += 1;
                has_exp = true;
                break;
            }
            _ => break,
        }
    }

    if !(has_dot || has_exp) {
        return Err(Reject);
    }

    if has_exp {
        let token_before_exp = if has_dot {
            Ok(input.advance(len - 1))
        } else {
            Err(Reject)
        };
        let mut has_sign = false;
        let mut has_exp_value = false;
        while let Some(&ch) = chars.peek() {
            match ch {
                '+' | '-' => {
                    if has_exp_value {
                        break;
                    }
                    if has_sign {
                        return token_before_exp;
                    }
                    chars.next();
                    len += 1;
                    has_sign = true;
                }
                '0'..='9' => {
                    chars.next();
                    len += 1;
                    has_exp_value = true;
                }
                '_' => {
                    chars.next();
                    len += 1;
                }
                _ => break,
            }
        }
        if !has_exp_value {
            return token_before_exp;
        }
    }

    Ok(input.advance(len))
}

fn int(input: Cursor) -> Result<Cursor, Reject> {
    let mut rest = digits(input)?;
    if let Some(ch) = rest.chars().next() {
        if is_ident_start(ch) {
            rest = ident_not_raw(rest)?.0;
        }
    }
    word_break(rest)
}

fn digits(mut input: Cursor) -> Result<Cursor, Reject> {
    let base = if input.starts_with("0x") {
        input = input.advance(2);
        16
    } else if input.starts_with("0o") {
        input = input.advance(2);
        8
    } else if input.starts_with("0b") {
        input = input.advance(2);
        2
    } else {
        10
    };

    let mut len = 0;
    let mut empty = true;
    for b in input.bytes() {
        match b {
            b'0'..=b'9' => {
                let digit = (b - b'0') as u64;
                if digit >= base {
                    return Err(Reject);
                }
            }
            b'a'..=b'f' => {
                let digit = 10 + (b - b'a') as u64;
                if digit >= base {
                    break;
                }
            }
            b'A'..=b'F' => {
                let digit = 10 + (b - b'A') as u64;
                if digit >= base {
                    break;
                }
            }
            b'_' => {
                if empty && base == 10 {
                    return Err(Reject);
                }
                len += 1;
                continue;
            }
            _ => break,
        };
        len += 1;
        empty = false;
    }
    if empty {
        Err(Reject)
    } else {
        Ok(input.advance(len))
    }
}

fn punct(input: Cursor) -> PResult<Punct> {
    let (rest, ch) = punct_char(input)?;
    if ch == '\'' {
        if ident_any(rest)?.0.starts_with("'") {
            Err(Reject)
        } else {
            Ok((rest, Punct::new('\'', Spacing::Joint)))
        }
    } else {
        let kind = match punct_char(rest) {
            Ok(_) => Spacing::Joint,
            Err(Reject) => Spacing::Alone,
        };
        Ok((rest, Punct::new(ch, kind)))
    }
}

fn punct_char(input: Cursor) -> PResult<char> {
    if input.starts_with("//") || input.starts_with("/*") {
        // Do not accept `/` of a comment as a punct.
        return Err(Reject);
    }

    let mut chars = input.chars();
    let first = match chars.next() {
        Some(ch) => ch,
        None => {
            return Err(Reject);
        }
    };
    let recognized = "~!@#$%^&*-=+|;:,<.>/?'";
    if recognized.contains(first) {
        Ok((input.advance(first.len_utf8()), first))
    } else {
        Err(Reject)
    }
}

fn doc_comment(input: Cursor) -> PResult<Vec<TokenTree>> {
    #[cfg(span_locations)]
    let lo = input.off;
    let (rest, (comment, inner)) = doc_comment_contents(input)?;
    let span = crate::Span::_new_stable(Span {
        #[cfg(span_locations)]
        lo,
        #[cfg(span_locations)]
        hi: rest.off,
    });

    let mut scan_for_bare_cr = comment;
    while let Some(cr) = scan_for_bare_cr.find('\r') {
        let rest = &scan_for_bare_cr[cr + 1..];
        if !rest.starts_with('\n') {
            return Err(Reject);
        }
        scan_for_bare_cr = rest;
    }

    let mut trees = Vec::new();
    trees.push(TokenTree::Punct(Punct::new('#', Spacing::Alone)));
    if inner {
        trees.push(Punct::new('!', Spacing::Alone).into());
    }
    let mut stream = vec![
        TokenTree::Ident(crate::Ident::new("doc", span)),
        TokenTree::Punct(Punct::new('=', Spacing::Alone)),
        TokenTree::Literal(crate::Literal::string(comment)),
    ];
    for tt in stream.iter_mut() {
        tt.set_span(span);
    }
    let group = Group::new(Delimiter::Bracket, stream.into_iter().collect());
    trees.push(crate::Group::_new_stable(group).into());
    for tt in trees.iter_mut() {
        tt.set_span(span);
    }
    Ok((rest, trees))
}

fn doc_comment_contents(input: Cursor) -> PResult<(&str, bool)> {
    if input.starts_with("//!") {
        let input = input.advance(3);
        let (input, s) = take_until_newline_or_eof(input);
        Ok((input, (s, true)))
    } else if input.starts_with("/*!") {
        let (input, s) = block_comment(input)?;
        Ok((input, (&s[3..s.len() - 2], true)))
    } else if input.starts_with("///") {
        let input = input.advance(3);
        if input.starts_with("/") {
            return Err(Reject);
        }
        let (input, s) = take_until_newline_or_eof(input);
        Ok((input, (s, false)))
    } else if input.starts_with("/**") && !input.rest[3..].starts_with('*') {
        let (input, s) = block_comment(input)?;
        Ok((input, (&s[3..s.len() - 2], false)))
    } else {
        Err(Reject)
    }
}

fn take_until_newline_or_eof(input: Cursor) -> (Cursor, &str) {
    let chars = input.char_indices();

    for (i, ch) in chars {
        if ch == '\n' {
            return (input.advance(i), &input.rest[..i]);
        } else if ch == '\r' && input.rest[i + 1..].starts_with('\n') {
            return (input.advance(i + 1), &input.rest[..i]);
        }
    }

    (input.advance(input.len()), input.rest)
}
