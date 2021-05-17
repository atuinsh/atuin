use crate::parse::{self, Cursor};
use crate::{Delimiter, Spacing, TokenTree};
#[cfg(span_locations)]
use std::cell::RefCell;
#[cfg(span_locations)]
use std::cmp;
use std::fmt::{self, Debug, Display};
use std::iter::FromIterator;
use std::mem;
use std::ops::RangeBounds;
#[cfg(procmacro2_semver_exempt)]
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::vec;
use unicode_xid::UnicodeXID;

/// Force use of proc-macro2's fallback implementation of the API for now, even
/// if the compiler's implementation is available.
pub fn force() {
    #[cfg(wrap_proc_macro)]
    crate::detection::force_fallback();
}

/// Resume using the compiler's implementation of the proc macro API if it is
/// available.
pub fn unforce() {
    #[cfg(wrap_proc_macro)]
    crate::detection::unforce_fallback();
}

#[derive(Clone)]
pub(crate) struct TokenStream {
    pub(crate) inner: Vec<TokenTree>,
}

#[derive(Debug)]
pub(crate) struct LexError {
    pub(crate) span: Span,
}

impl LexError {
    pub(crate) fn span(&self) -> Span {
        self.span
    }
}

impl TokenStream {
    pub fn new() -> TokenStream {
        TokenStream { inner: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.inner.len() == 0
    }

    fn take_inner(&mut self) -> Vec<TokenTree> {
        mem::replace(&mut self.inner, Vec::new())
    }

    fn push_token(&mut self, token: TokenTree) {
        // https://github.com/alexcrichton/proc-macro2/issues/235
        match token {
            #[cfg(not(no_bind_by_move_pattern_guard))]
            TokenTree::Literal(crate::Literal {
                #[cfg(wrap_proc_macro)]
                    inner: crate::imp::Literal::Fallback(literal),
                #[cfg(not(wrap_proc_macro))]
                    inner: literal,
                ..
            }) if literal.text.starts_with('-') => {
                push_negative_literal(self, literal);
            }
            #[cfg(no_bind_by_move_pattern_guard)]
            TokenTree::Literal(crate::Literal {
                #[cfg(wrap_proc_macro)]
                    inner: crate::imp::Literal::Fallback(literal),
                #[cfg(not(wrap_proc_macro))]
                    inner: literal,
                ..
            }) => {
                if literal.text.starts_with('-') {
                    push_negative_literal(self, literal);
                } else {
                    self.inner
                        .push(TokenTree::Literal(crate::Literal::_new_stable(literal)));
                }
            }
            _ => self.inner.push(token),
        }

        #[cold]
        fn push_negative_literal(stream: &mut TokenStream, mut literal: Literal) {
            literal.text.remove(0);
            let mut punct = crate::Punct::new('-', Spacing::Alone);
            punct.set_span(crate::Span::_new_stable(literal.span));
            stream.inner.push(TokenTree::Punct(punct));
            stream
                .inner
                .push(TokenTree::Literal(crate::Literal::_new_stable(literal)));
        }
    }
}

// Nonrecursive to prevent stack overflow.
impl Drop for TokenStream {
    fn drop(&mut self) {
        while let Some(token) = self.inner.pop() {
            let group = match token {
                TokenTree::Group(group) => group.inner,
                _ => continue,
            };
            #[cfg(wrap_proc_macro)]
            let group = match group {
                crate::imp::Group::Fallback(group) => group,
                _ => continue,
            };
            let mut group = group;
            self.inner.extend(group.stream.take_inner());
        }
    }
}

#[cfg(span_locations)]
fn get_cursor(src: &str) -> Cursor {
    // Create a dummy file & add it to the source map
    SOURCE_MAP.with(|cm| {
        let mut cm = cm.borrow_mut();
        let name = format!("<parsed string {}>", cm.files.len());
        let span = cm.add_file(&name, src);
        Cursor {
            rest: src,
            off: span.lo,
        }
    })
}

#[cfg(not(span_locations))]
fn get_cursor(src: &str) -> Cursor {
    Cursor { rest: src }
}

impl FromStr for TokenStream {
    type Err = LexError;

    fn from_str(src: &str) -> Result<TokenStream, LexError> {
        // Create a dummy file & add it to the source map
        let cursor = get_cursor(src);

        parse::token_stream(cursor)
    }
}

impl Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("cannot parse string into token stream")
    }
}

impl Display for TokenStream {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut joint = false;
        for (i, tt) in self.inner.iter().enumerate() {
            if i != 0 && !joint {
                write!(f, " ")?;
            }
            joint = false;
            match tt {
                TokenTree::Group(tt) => Display::fmt(tt, f),
                TokenTree::Ident(tt) => Display::fmt(tt, f),
                TokenTree::Punct(tt) => {
                    joint = tt.spacing() == Spacing::Joint;
                    Display::fmt(tt, f)
                }
                TokenTree::Literal(tt) => Display::fmt(tt, f),
            }?
        }

        Ok(())
    }
}

impl Debug for TokenStream {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("TokenStream ")?;
        f.debug_list().entries(self.clone()).finish()
    }
}

#[cfg(use_proc_macro)]
impl From<proc_macro::TokenStream> for TokenStream {
    fn from(inner: proc_macro::TokenStream) -> TokenStream {
        inner
            .to_string()
            .parse()
            .expect("compiler token stream parse failed")
    }
}

#[cfg(use_proc_macro)]
impl From<TokenStream> for proc_macro::TokenStream {
    fn from(inner: TokenStream) -> proc_macro::TokenStream {
        inner
            .to_string()
            .parse()
            .expect("failed to parse to compiler tokens")
    }
}

impl From<TokenTree> for TokenStream {
    fn from(tree: TokenTree) -> TokenStream {
        let mut stream = TokenStream::new();
        stream.push_token(tree);
        stream
    }
}

impl FromIterator<TokenTree> for TokenStream {
    fn from_iter<I: IntoIterator<Item = TokenTree>>(tokens: I) -> Self {
        let mut stream = TokenStream::new();
        stream.extend(tokens);
        stream
    }
}

impl FromIterator<TokenStream> for TokenStream {
    fn from_iter<I: IntoIterator<Item = TokenStream>>(streams: I) -> Self {
        let mut v = Vec::new();

        for mut stream in streams {
            v.extend(stream.take_inner());
        }

        TokenStream { inner: v }
    }
}

impl Extend<TokenTree> for TokenStream {
    fn extend<I: IntoIterator<Item = TokenTree>>(&mut self, tokens: I) {
        tokens.into_iter().for_each(|token| self.push_token(token));
    }
}

impl Extend<TokenStream> for TokenStream {
    fn extend<I: IntoIterator<Item = TokenStream>>(&mut self, streams: I) {
        self.inner.extend(streams.into_iter().flatten());
    }
}

pub(crate) type TokenTreeIter = vec::IntoIter<TokenTree>;

impl IntoIterator for TokenStream {
    type Item = TokenTree;
    type IntoIter = TokenTreeIter;

    fn into_iter(mut self) -> TokenTreeIter {
        self.take_inner().into_iter()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SourceFile {
    path: PathBuf,
}

impl SourceFile {
    /// Get the path to this source file as a string.
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }

    pub fn is_real(&self) -> bool {
        // XXX(nika): Support real files in the future?
        false
    }
}

impl Debug for SourceFile {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("SourceFile")
            .field("path", &self.path())
            .field("is_real", &self.is_real())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LineColumn {
    pub line: usize,
    pub column: usize,
}

#[cfg(span_locations)]
thread_local! {
    static SOURCE_MAP: RefCell<SourceMap> = RefCell::new(SourceMap {
        // NOTE: We start with a single dummy file which all call_site() and
        // def_site() spans reference.
        files: vec![FileInfo {
            #[cfg(procmacro2_semver_exempt)]
            name: "<unspecified>".to_owned(),
            span: Span { lo: 0, hi: 0 },
            lines: vec![0],
        }],
    });
}

#[cfg(span_locations)]
struct FileInfo {
    #[cfg(procmacro2_semver_exempt)]
    name: String,
    span: Span,
    lines: Vec<usize>,
}

#[cfg(span_locations)]
impl FileInfo {
    fn offset_line_column(&self, offset: usize) -> LineColumn {
        assert!(self.span_within(Span {
            lo: offset as u32,
            hi: offset as u32
        }));
        let offset = offset - self.span.lo as usize;
        match self.lines.binary_search(&offset) {
            Ok(found) => LineColumn {
                line: found + 1,
                column: 0,
            },
            Err(idx) => LineColumn {
                line: idx,
                column: offset - self.lines[idx - 1],
            },
        }
    }

    fn span_within(&self, span: Span) -> bool {
        span.lo >= self.span.lo && span.hi <= self.span.hi
    }
}

/// Computes the offsets of each line in the given source string
/// and the total number of characters
#[cfg(span_locations)]
fn lines_offsets(s: &str) -> (usize, Vec<usize>) {
    let mut lines = vec![0];
    let mut total = 0;

    for ch in s.chars() {
        total += 1;
        if ch == '\n' {
            lines.push(total);
        }
    }

    (total, lines)
}

#[cfg(span_locations)]
struct SourceMap {
    files: Vec<FileInfo>,
}

#[cfg(span_locations)]
impl SourceMap {
    fn next_start_pos(&self) -> u32 {
        // Add 1 so there's always space between files.
        //
        // We'll always have at least 1 file, as we initialize our files list
        // with a dummy file.
        self.files.last().unwrap().span.hi + 1
    }

    fn add_file(&mut self, name: &str, src: &str) -> Span {
        let (len, lines) = lines_offsets(src);
        let lo = self.next_start_pos();
        // XXX(nika): Shouild we bother doing a checked cast or checked add here?
        let span = Span {
            lo,
            hi: lo + (len as u32),
        };

        self.files.push(FileInfo {
            #[cfg(procmacro2_semver_exempt)]
            name: name.to_owned(),
            span,
            lines,
        });

        #[cfg(not(procmacro2_semver_exempt))]
        let _ = name;

        span
    }

    fn fileinfo(&self, span: Span) -> &FileInfo {
        for file in &self.files {
            if file.span_within(span) {
                return file;
            }
        }
        panic!("Invalid span with no related FileInfo!");
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    #[cfg(span_locations)]
    pub(crate) lo: u32,
    #[cfg(span_locations)]
    pub(crate) hi: u32,
}

impl Span {
    #[cfg(not(span_locations))]
    pub fn call_site() -> Span {
        Span {}
    }

    #[cfg(span_locations)]
    pub fn call_site() -> Span {
        Span { lo: 0, hi: 0 }
    }

    #[cfg(hygiene)]
    pub fn mixed_site() -> Span {
        Span::call_site()
    }

    #[cfg(procmacro2_semver_exempt)]
    pub fn def_site() -> Span {
        Span::call_site()
    }

    pub fn resolved_at(&self, _other: Span) -> Span {
        // Stable spans consist only of line/column information, so
        // `resolved_at` and `located_at` only select which span the
        // caller wants line/column information from.
        *self
    }

    pub fn located_at(&self, other: Span) -> Span {
        other
    }

    #[cfg(procmacro2_semver_exempt)]
    pub fn source_file(&self) -> SourceFile {
        SOURCE_MAP.with(|cm| {
            let cm = cm.borrow();
            let fi = cm.fileinfo(*self);
            SourceFile {
                path: Path::new(&fi.name).to_owned(),
            }
        })
    }

    #[cfg(span_locations)]
    pub fn start(&self) -> LineColumn {
        SOURCE_MAP.with(|cm| {
            let cm = cm.borrow();
            let fi = cm.fileinfo(*self);
            fi.offset_line_column(self.lo as usize)
        })
    }

    #[cfg(span_locations)]
    pub fn end(&self) -> LineColumn {
        SOURCE_MAP.with(|cm| {
            let cm = cm.borrow();
            let fi = cm.fileinfo(*self);
            fi.offset_line_column(self.hi as usize)
        })
    }

    #[cfg(not(span_locations))]
    pub fn join(&self, _other: Span) -> Option<Span> {
        Some(Span {})
    }

    #[cfg(span_locations)]
    pub fn join(&self, other: Span) -> Option<Span> {
        SOURCE_MAP.with(|cm| {
            let cm = cm.borrow();
            // If `other` is not within the same FileInfo as us, return None.
            if !cm.fileinfo(*self).span_within(other) {
                return None;
            }
            Some(Span {
                lo: cmp::min(self.lo, other.lo),
                hi: cmp::max(self.hi, other.hi),
            })
        })
    }

    #[cfg(not(span_locations))]
    fn first_byte(self) -> Self {
        self
    }

    #[cfg(span_locations)]
    fn first_byte(self) -> Self {
        Span {
            lo: self.lo,
            hi: cmp::min(self.lo.saturating_add(1), self.hi),
        }
    }

    #[cfg(not(span_locations))]
    fn last_byte(self) -> Self {
        self
    }

    #[cfg(span_locations)]
    fn last_byte(self) -> Self {
        Span {
            lo: cmp::max(self.hi.saturating_sub(1), self.lo),
            hi: self.hi,
        }
    }
}

impl Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        #[cfg(span_locations)]
        return write!(f, "bytes({}..{})", self.lo, self.hi);

        #[cfg(not(span_locations))]
        write!(f, "Span")
    }
}

pub(crate) fn debug_span_field_if_nontrivial(debug: &mut fmt::DebugStruct, span: Span) {
    #[cfg(span_locations)]
    {
        if span.lo == 0 && span.hi == 0 {
            return;
        }
    }

    if cfg!(span_locations) {
        debug.field("span", &span);
    }
}

#[derive(Clone)]
pub(crate) struct Group {
    delimiter: Delimiter,
    stream: TokenStream,
    span: Span,
}

impl Group {
    pub fn new(delimiter: Delimiter, stream: TokenStream) -> Group {
        Group {
            delimiter,
            stream,
            span: Span::call_site(),
        }
    }

    pub fn delimiter(&self) -> Delimiter {
        self.delimiter
    }

    pub fn stream(&self) -> TokenStream {
        self.stream.clone()
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn span_open(&self) -> Span {
        self.span.first_byte()
    }

    pub fn span_close(&self) -> Span {
        self.span.last_byte()
    }

    pub fn set_span(&mut self, span: Span) {
        self.span = span;
    }
}

impl Display for Group {
    // We attempt to match libproc_macro's formatting.
    // Empty parens: ()
    // Nonempty parens: (...)
    // Empty brackets: []
    // Nonempty brackets: [...]
    // Empty braces: { }
    // Nonempty braces: { ... }
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let (open, close) = match self.delimiter {
            Delimiter::Parenthesis => ("(", ")"),
            Delimiter::Brace => ("{ ", "}"),
            Delimiter::Bracket => ("[", "]"),
            Delimiter::None => ("", ""),
        };

        f.write_str(open)?;
        Display::fmt(&self.stream, f)?;
        if self.delimiter == Delimiter::Brace && !self.stream.inner.is_empty() {
            f.write_str(" ")?;
        }
        f.write_str(close)?;

        Ok(())
    }
}

impl Debug for Group {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let mut debug = fmt.debug_struct("Group");
        debug.field("delimiter", &self.delimiter);
        debug.field("stream", &self.stream);
        debug_span_field_if_nontrivial(&mut debug, self.span);
        debug.finish()
    }
}

#[derive(Clone)]
pub(crate) struct Ident {
    sym: String,
    span: Span,
    raw: bool,
}

impl Ident {
    fn _new(string: &str, raw: bool, span: Span) -> Ident {
        validate_ident(string);

        Ident {
            sym: string.to_owned(),
            span,
            raw,
        }
    }

    pub fn new(string: &str, span: Span) -> Ident {
        Ident::_new(string, false, span)
    }

    pub fn new_raw(string: &str, span: Span) -> Ident {
        Ident::_new(string, true, span)
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn set_span(&mut self, span: Span) {
        self.span = span;
    }
}

pub(crate) fn is_ident_start(c: char) -> bool {
    ('a' <= c && c <= 'z')
        || ('A' <= c && c <= 'Z')
        || c == '_'
        || (c > '\x7f' && UnicodeXID::is_xid_start(c))
}

pub(crate) fn is_ident_continue(c: char) -> bool {
    ('a' <= c && c <= 'z')
        || ('A' <= c && c <= 'Z')
        || c == '_'
        || ('0' <= c && c <= '9')
        || (c > '\x7f' && UnicodeXID::is_xid_continue(c))
}

fn validate_ident(string: &str) {
    let validate = string;
    if validate.is_empty() {
        panic!("Ident is not allowed to be empty; use Option<Ident>");
    }

    if validate.bytes().all(|digit| digit >= b'0' && digit <= b'9') {
        panic!("Ident cannot be a number; use Literal instead");
    }

    fn ident_ok(string: &str) -> bool {
        let mut chars = string.chars();
        let first = chars.next().unwrap();
        if !is_ident_start(first) {
            return false;
        }
        for ch in chars {
            if !is_ident_continue(ch) {
                return false;
            }
        }
        true
    }

    if !ident_ok(validate) {
        panic!("{:?} is not a valid Ident", string);
    }
}

impl PartialEq for Ident {
    fn eq(&self, other: &Ident) -> bool {
        self.sym == other.sym && self.raw == other.raw
    }
}

impl<T> PartialEq<T> for Ident
where
    T: ?Sized + AsRef<str>,
{
    fn eq(&self, other: &T) -> bool {
        let other = other.as_ref();
        if self.raw {
            other.starts_with("r#") && self.sym == other[2..]
        } else {
            self.sym == other
        }
    }
}

impl Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.raw {
            f.write_str("r#")?;
        }
        Display::fmt(&self.sym, f)
    }
}

impl Debug for Ident {
    // Ident(proc_macro), Ident(r#union)
    #[cfg(not(span_locations))]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut debug = f.debug_tuple("Ident");
        debug.field(&format_args!("{}", self));
        debug.finish()
    }

    // Ident {
    //     sym: proc_macro,
    //     span: bytes(128..138)
    // }
    #[cfg(span_locations)]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut debug = f.debug_struct("Ident");
        debug.field("sym", &format_args!("{}", self));
        debug_span_field_if_nontrivial(&mut debug, self.span);
        debug.finish()
    }
}

#[derive(Clone)]
pub(crate) struct Literal {
    text: String,
    span: Span,
}

macro_rules! suffixed_numbers {
    ($($name:ident => $kind:ident,)*) => ($(
        pub fn $name(n: $kind) -> Literal {
            Literal::_new(format!(concat!("{}", stringify!($kind)), n))
        }
    )*)
}

macro_rules! unsuffixed_numbers {
    ($($name:ident => $kind:ident,)*) => ($(
        pub fn $name(n: $kind) -> Literal {
            Literal::_new(n.to_string())
        }
    )*)
}

impl Literal {
    pub(crate) fn _new(text: String) -> Literal {
        Literal {
            text,
            span: Span::call_site(),
        }
    }

    suffixed_numbers! {
        u8_suffixed => u8,
        u16_suffixed => u16,
        u32_suffixed => u32,
        u64_suffixed => u64,
        u128_suffixed => u128,
        usize_suffixed => usize,
        i8_suffixed => i8,
        i16_suffixed => i16,
        i32_suffixed => i32,
        i64_suffixed => i64,
        i128_suffixed => i128,
        isize_suffixed => isize,

        f32_suffixed => f32,
        f64_suffixed => f64,
    }

    unsuffixed_numbers! {
        u8_unsuffixed => u8,
        u16_unsuffixed => u16,
        u32_unsuffixed => u32,
        u64_unsuffixed => u64,
        u128_unsuffixed => u128,
        usize_unsuffixed => usize,
        i8_unsuffixed => i8,
        i16_unsuffixed => i16,
        i32_unsuffixed => i32,
        i64_unsuffixed => i64,
        i128_unsuffixed => i128,
        isize_unsuffixed => isize,
    }

    pub fn f32_unsuffixed(f: f32) -> Literal {
        let mut s = f.to_string();
        if !s.contains('.') {
            s.push_str(".0");
        }
        Literal::_new(s)
    }

    pub fn f64_unsuffixed(f: f64) -> Literal {
        let mut s = f.to_string();
        if !s.contains('.') {
            s.push_str(".0");
        }
        Literal::_new(s)
    }

    pub fn string(t: &str) -> Literal {
        let mut text = String::with_capacity(t.len() + 2);
        text.push('"');
        for c in t.chars() {
            if c == '\'' {
                // escape_debug turns this into "\'" which is unnecessary.
                text.push(c);
            } else {
                text.extend(c.escape_debug());
            }
        }
        text.push('"');
        Literal::_new(text)
    }

    pub fn character(t: char) -> Literal {
        let mut text = String::new();
        text.push('\'');
        if t == '"' {
            // escape_debug turns this into '\"' which is unnecessary.
            text.push(t);
        } else {
            text.extend(t.escape_debug());
        }
        text.push('\'');
        Literal::_new(text)
    }

    pub fn byte_string(bytes: &[u8]) -> Literal {
        let mut escaped = "b\"".to_string();
        for b in bytes {
            #[allow(clippy::match_overlapping_arm)]
            match *b {
                b'\0' => escaped.push_str(r"\0"),
                b'\t' => escaped.push_str(r"\t"),
                b'\n' => escaped.push_str(r"\n"),
                b'\r' => escaped.push_str(r"\r"),
                b'"' => escaped.push_str("\\\""),
                b'\\' => escaped.push_str("\\\\"),
                b'\x20'..=b'\x7E' => escaped.push(*b as char),
                _ => escaped.push_str(&format!("\\x{:02X}", b)),
            }
        }
        escaped.push('"');
        Literal::_new(escaped)
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn set_span(&mut self, span: Span) {
        self.span = span;
    }

    pub fn subspan<R: RangeBounds<usize>>(&self, _range: R) -> Option<Span> {
        None
    }
}

impl Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Display::fmt(&self.text, f)
    }
}

impl Debug for Literal {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let mut debug = fmt.debug_struct("Literal");
        debug.field("lit", &format_args!("{}", self.text));
        debug_span_field_if_nontrivial(&mut debug, self.span);
        debug.finish()
    }
}
