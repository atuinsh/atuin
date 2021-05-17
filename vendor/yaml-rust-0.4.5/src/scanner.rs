use std::collections::VecDeque;
use std::error::Error;
use std::{char, fmt};

#[derive(Clone, Copy, PartialEq, Debug, Eq)]
pub enum TEncoding {
    Utf8,
}

#[derive(Clone, Copy, PartialEq, Debug, Eq)]
pub enum TScalarStyle {
    Any,
    Plain,
    SingleQuoted,
    DoubleQuoted,

    Literal,
    Foled,
}

#[derive(Clone, Copy, PartialEq, Debug, Eq)]
pub struct Marker {
    index: usize,
    line: usize,
    col: usize,
}

impl Marker {
    fn new(index: usize, line: usize, col: usize) -> Marker {
        Marker { index, line, col }
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn line(&self) -> usize {
        self.line
    }

    pub fn col(&self) -> usize {
        self.col
    }
}

#[derive(Clone, PartialEq, Debug, Eq)]
pub struct ScanError {
    mark: Marker,
    info: String,
}

impl ScanError {
    pub fn new(loc: Marker, info: &str) -> ScanError {
        ScanError {
            mark: loc,
            info: info.to_owned(),
        }
    }

    pub fn marker(&self) -> &Marker {
        &self.mark
    }
}

impl Error for ScanError {
    fn description(&self) -> &str {
        self.info.as_ref()
    }

    fn cause(&self) -> Option<&dyn Error> {
        None
    }
}

impl fmt::Display for ScanError {
    // col starts from 0
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(
            formatter,
            "{} at line {} column {}",
            self.info,
            self.mark.line,
            self.mark.col + 1
        )
    }
}

#[derive(Clone, PartialEq, Debug, Eq)]
pub enum TokenType {
    NoToken,
    StreamStart(TEncoding),
    StreamEnd,
    /// major, minor
    VersionDirective(u32, u32),
    /// handle, prefix
    TagDirective(String, String),
    DocumentStart,
    DocumentEnd,
    BlockSequenceStart,
    BlockMappingStart,
    BlockEnd,
    FlowSequenceStart,
    FlowSequenceEnd,
    FlowMappingStart,
    FlowMappingEnd,
    BlockEntry,
    FlowEntry,
    Key,
    Value,
    Alias(String),
    Anchor(String),
    /// handle, suffix
    Tag(String, String),
    Scalar(TScalarStyle, String),
}

#[derive(Clone, PartialEq, Debug, Eq)]
pub struct Token(pub Marker, pub TokenType);

#[derive(Clone, PartialEq, Debug, Eq)]
struct SimpleKey {
    possible: bool,
    required: bool,
    token_number: usize,
    mark: Marker,
}

impl SimpleKey {
    fn new(mark: Marker) -> SimpleKey {
        SimpleKey {
            possible: false,
            required: false,
            token_number: 0,
            mark,
        }
    }
}

#[derive(Debug)]
pub struct Scanner<T> {
    rdr: T,
    mark: Marker,
    tokens: VecDeque<Token>,
    buffer: VecDeque<char>,
    error: Option<ScanError>,

    stream_start_produced: bool,
    stream_end_produced: bool,
    adjacent_value_allowed_at: usize,
    simple_key_allowed: bool,
    simple_keys: Vec<SimpleKey>,
    indent: isize,
    indents: Vec<isize>,
    flow_level: u8,
    tokens_parsed: usize,
    token_available: bool,
}

impl<T: Iterator<Item = char>> Iterator for Scanner<T> {
    type Item = Token;
    fn next(&mut self) -> Option<Token> {
        if self.error.is_some() {
            return None;
        }
        match self.next_token() {
            Ok(tok) => tok,
            Err(e) => {
                self.error = Some(e);
                None
            }
        }
    }
}

#[inline]
fn is_z(c: char) -> bool {
    c == '\0'
}
#[inline]
fn is_break(c: char) -> bool {
    c == '\n' || c == '\r'
}
#[inline]
fn is_breakz(c: char) -> bool {
    is_break(c) || is_z(c)
}
#[inline]
fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}
#[inline]
fn is_blankz(c: char) -> bool {
    is_blank(c) || is_breakz(c)
}
#[inline]
fn is_digit(c: char) -> bool {
    c >= '0' && c <= '9'
}
#[inline]
fn is_alpha(c: char) -> bool {
    match c {
        '0'..='9' | 'a'..='z' | 'A'..='Z' => true,
        '_' | '-' => true,
        _ => false,
    }
}
#[inline]
fn is_hex(c: char) -> bool {
    (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F')
}
#[inline]
fn as_hex(c: char) -> u32 {
    match c {
        '0'..='9' => (c as u32) - ('0' as u32),
        'a'..='f' => (c as u32) - ('a' as u32) + 10,
        'A'..='F' => (c as u32) - ('A' as u32) + 10,
        _ => unreachable!(),
    }
}
#[inline]
fn is_flow(c: char) -> bool {
    match c {
        ',' | '[' | ']' | '{' | '}' => true,
        _ => false,
    }
}

pub type ScanResult = Result<(), ScanError>;

impl<T: Iterator<Item = char>> Scanner<T> {
    /// Creates the YAML tokenizer.
    pub fn new(rdr: T) -> Scanner<T> {
        Scanner {
            rdr,
            buffer: VecDeque::new(),
            mark: Marker::new(0, 1, 0),
            tokens: VecDeque::new(),
            error: None,

            stream_start_produced: false,
            stream_end_produced: false,
            adjacent_value_allowed_at: 0,
            simple_key_allowed: true,
            simple_keys: Vec::new(),
            indent: -1,
            indents: Vec::new(),
            flow_level: 0,
            tokens_parsed: 0,
            token_available: false,
        }
    }
    #[inline]
    pub fn get_error(&self) -> Option<ScanError> {
        match self.error {
            None => None,
            Some(ref e) => Some(e.clone()),
        }
    }

    #[inline]
    fn lookahead(&mut self, count: usize) {
        if self.buffer.len() >= count {
            return;
        }
        for _ in 0..(count - self.buffer.len()) {
            self.buffer.push_back(self.rdr.next().unwrap_or('\0'));
        }
    }
    #[inline]
    fn skip(&mut self) {
        let c = self.buffer.pop_front().unwrap();

        self.mark.index += 1;
        if c == '\n' {
            self.mark.line += 1;
            self.mark.col = 0;
        } else {
            self.mark.col += 1;
        }
    }
    #[inline]
    fn skip_line(&mut self) {
        if self.buffer[0] == '\r' && self.buffer[1] == '\n' {
            self.skip();
            self.skip();
        } else if is_break(self.buffer[0]) {
            self.skip();
        }
    }
    #[inline]
    fn ch(&self) -> char {
        self.buffer[0]
    }
    #[inline]
    fn ch_is(&self, c: char) -> bool {
        self.buffer[0] == c
    }
    #[allow(dead_code)]
    #[inline]
    fn eof(&self) -> bool {
        self.ch_is('\0')
    }
    #[inline]
    pub fn stream_started(&self) -> bool {
        self.stream_start_produced
    }
    #[inline]
    pub fn stream_ended(&self) -> bool {
        self.stream_end_produced
    }
    #[inline]
    pub fn mark(&self) -> Marker {
        self.mark
    }
    #[inline]
    fn read_break(&mut self, s: &mut String) {
        if self.buffer[0] == '\r' && self.buffer[1] == '\n' {
            s.push('\n');
            self.skip();
            self.skip();
        } else if self.buffer[0] == '\r' || self.buffer[0] == '\n' {
            s.push('\n');
            self.skip();
        } else {
            unreachable!();
        }
    }
    fn insert_token(&mut self, pos: usize, tok: Token) {
        let old_len = self.tokens.len();
        assert!(pos <= old_len);
        self.tokens.push_back(tok);
        for i in 0..old_len - pos {
            self.tokens.swap(old_len - i, old_len - i - 1);
        }
    }
    fn allow_simple_key(&mut self) {
        self.simple_key_allowed = true;
    }
    fn disallow_simple_key(&mut self) {
        self.simple_key_allowed = false;
    }

    pub fn fetch_next_token(&mut self) -> ScanResult {
        self.lookahead(1);
        // println!("--> fetch_next_token Cur {:?} {:?}", self.mark, self.ch());

        if !self.stream_start_produced {
            self.fetch_stream_start();
            return Ok(());
        }
        self.skip_to_next_token();

        self.stale_simple_keys()?;

        let mark = self.mark;
        self.unroll_indent(mark.col as isize);

        self.lookahead(4);

        if is_z(self.ch()) {
            self.fetch_stream_end()?;
            return Ok(());
        }

        // Is it a directive?
        if self.mark.col == 0 && self.ch_is('%') {
            return self.fetch_directive();
        }

        if self.mark.col == 0
            && self.buffer[0] == '-'
            && self.buffer[1] == '-'
            && self.buffer[2] == '-'
            && is_blankz(self.buffer[3])
        {
            self.fetch_document_indicator(TokenType::DocumentStart)?;
            return Ok(());
        }

        if self.mark.col == 0
            && self.buffer[0] == '.'
            && self.buffer[1] == '.'
            && self.buffer[2] == '.'
            && is_blankz(self.buffer[3])
        {
            self.fetch_document_indicator(TokenType::DocumentEnd)?;
            return Ok(());
        }

        let c = self.buffer[0];
        let nc = self.buffer[1];
        match c {
            '[' => self.fetch_flow_collection_start(TokenType::FlowSequenceStart),
            '{' => self.fetch_flow_collection_start(TokenType::FlowMappingStart),
            ']' => self.fetch_flow_collection_end(TokenType::FlowSequenceEnd),
            '}' => self.fetch_flow_collection_end(TokenType::FlowMappingEnd),
            ',' => self.fetch_flow_entry(),
            '-' if is_blankz(nc) => self.fetch_block_entry(),
            '?' if is_blankz(nc) => self.fetch_key(),
            ':' if is_blankz(nc)
                || (self.flow_level > 0
                    && (is_flow(nc) || self.mark.index == self.adjacent_value_allowed_at)) =>
            {
                self.fetch_value()
            }
            // Is it an alias?
            '*' => self.fetch_anchor(true),
            // Is it an anchor?
            '&' => self.fetch_anchor(false),
            '!' => self.fetch_tag(),
            // Is it a literal scalar?
            '|' if self.flow_level == 0 => self.fetch_block_scalar(true),
            // Is it a folded scalar?
            '>' if self.flow_level == 0 => self.fetch_block_scalar(false),
            '\'' => self.fetch_flow_scalar(true),
            '"' => self.fetch_flow_scalar(false),
            // plain scalar
            '-' if !is_blankz(nc) => self.fetch_plain_scalar(),
            ':' | '?' if !is_blankz(nc) && self.flow_level == 0 => self.fetch_plain_scalar(),
            '%' | '@' | '`' => Err(ScanError::new(
                self.mark,
                &format!("unexpected character: `{}'", c),
            )),
            _ => self.fetch_plain_scalar(),
        }
    }

    pub fn next_token(&mut self) -> Result<Option<Token>, ScanError> {
        if self.stream_end_produced {
            return Ok(None);
        }

        if !self.token_available {
            self.fetch_more_tokens()?;
        }
        let t = self.tokens.pop_front().unwrap();
        self.token_available = false;
        self.tokens_parsed += 1;

        if let TokenType::StreamEnd = t.1 {
            self.stream_end_produced = true;
        }
        Ok(Some(t))
    }

    pub fn fetch_more_tokens(&mut self) -> ScanResult {
        let mut need_more;
        loop {
            need_more = false;
            if self.tokens.is_empty() {
                need_more = true;
            } else {
                self.stale_simple_keys()?;
                for sk in &self.simple_keys {
                    if sk.possible && sk.token_number == self.tokens_parsed {
                        need_more = true;
                        break;
                    }
                }
            }

            if !need_more {
                break;
            }
            self.fetch_next_token()?;
        }
        self.token_available = true;

        Ok(())
    }

    fn stale_simple_keys(&mut self) -> ScanResult {
        for sk in &mut self.simple_keys {
            if sk.possible
                && (sk.mark.line < self.mark.line || sk.mark.index + 1024 < self.mark.index)
            {
                if sk.required {
                    return Err(ScanError::new(self.mark, "simple key expect ':'"));
                }
                sk.possible = false;
            }
        }
        Ok(())
    }

    fn skip_to_next_token(&mut self) {
        loop {
            self.lookahead(1);
            // TODO(chenyh) BOM
            match self.ch() {
                ' ' => self.skip(),
                '\t' if self.flow_level > 0 || !self.simple_key_allowed => self.skip(),
                '\n' | '\r' => {
                    self.lookahead(2);
                    self.skip_line();
                    if self.flow_level == 0 {
                        self.allow_simple_key();
                    }
                }
                '#' => {
                    while !is_breakz(self.ch()) {
                        self.skip();
                        self.lookahead(1);
                    }
                }
                _ => break,
            }
        }
    }

    fn fetch_stream_start(&mut self) {
        let mark = self.mark;
        self.indent = -1;
        self.stream_start_produced = true;
        self.allow_simple_key();
        self.tokens
            .push_back(Token(mark, TokenType::StreamStart(TEncoding::Utf8)));
        self.simple_keys.push(SimpleKey::new(Marker::new(0, 0, 0)));
    }

    fn fetch_stream_end(&mut self) -> ScanResult {
        // force new line
        if self.mark.col != 0 {
            self.mark.col = 0;
            self.mark.line += 1;
        }

        self.unroll_indent(-1);
        self.remove_simple_key()?;
        self.disallow_simple_key();

        self.tokens
            .push_back(Token(self.mark, TokenType::StreamEnd));
        Ok(())
    }

    fn fetch_directive(&mut self) -> ScanResult {
        self.unroll_indent(-1);
        self.remove_simple_key()?;

        self.disallow_simple_key();

        let tok = self.scan_directive()?;

        self.tokens.push_back(tok);

        Ok(())
    }

    fn scan_directive(&mut self) -> Result<Token, ScanError> {
        let start_mark = self.mark;
        self.skip();

        let name = self.scan_directive_name()?;
        let tok = match name.as_ref() {
            "YAML" => self.scan_version_directive_value(&start_mark)?,
            "TAG" => self.scan_tag_directive_value(&start_mark)?,
            // XXX This should be a warning instead of an error
            _ => {
                // skip current line
                self.lookahead(1);
                while !is_breakz(self.ch()) {
                    self.skip();
                    self.lookahead(1);
                }
                // XXX return an empty TagDirective token
                Token(
                    start_mark,
                    TokenType::TagDirective(String::new(), String::new()),
                )
                // return Err(ScanError::new(start_mark,
                //     "while scanning a directive, found unknown directive name"))
            }
        };
        self.lookahead(1);

        while is_blank(self.ch()) {
            self.skip();
            self.lookahead(1);
        }

        if self.ch() == '#' {
            while !is_breakz(self.ch()) {
                self.skip();
                self.lookahead(1);
            }
        }

        if !is_breakz(self.ch()) {
            return Err(ScanError::new(
                start_mark,
                "while scanning a directive, did not find expected comment or line break",
            ));
        }

        // Eat a line break
        if is_break(self.ch()) {
            self.lookahead(2);
            self.skip_line();
        }

        Ok(tok)
    }

    fn scan_version_directive_value(&mut self, mark: &Marker) -> Result<Token, ScanError> {
        self.lookahead(1);

        while is_blank(self.ch()) {
            self.skip();
            self.lookahead(1);
        }

        let major = self.scan_version_directive_number(mark)?;

        if self.ch() != '.' {
            return Err(ScanError::new(
                *mark,
                "while scanning a YAML directive, did not find expected digit or '.' character",
            ));
        }

        self.skip();

        let minor = self.scan_version_directive_number(mark)?;

        Ok(Token(*mark, TokenType::VersionDirective(major, minor)))
    }

    fn scan_directive_name(&mut self) -> Result<String, ScanError> {
        let start_mark = self.mark;
        let mut string = String::new();
        self.lookahead(1);
        while is_alpha(self.ch()) {
            string.push(self.ch());
            self.skip();
            self.lookahead(1);
        }

        if string.is_empty() {
            return Err(ScanError::new(
                start_mark,
                "while scanning a directive, could not find expected directive name",
            ));
        }

        if !is_blankz(self.ch()) {
            return Err(ScanError::new(
                start_mark,
                "while scanning a directive, found unexpected non-alphabetical character",
            ));
        }

        Ok(string)
    }

    fn scan_version_directive_number(&mut self, mark: &Marker) -> Result<u32, ScanError> {
        let mut val = 0u32;
        let mut length = 0usize;
        self.lookahead(1);
        while is_digit(self.ch()) {
            if length + 1 > 9 {
                return Err(ScanError::new(
                    *mark,
                    "while scanning a YAML directive, found extremely long version number",
                ));
            }
            length += 1;
            val = val * 10 + ((self.ch() as u32) - ('0' as u32));
            self.skip();
            self.lookahead(1);
        }

        if length == 0 {
            return Err(ScanError::new(
                *mark,
                "while scanning a YAML directive, did not find expected version number",
            ));
        }

        Ok(val)
    }

    fn scan_tag_directive_value(&mut self, mark: &Marker) -> Result<Token, ScanError> {
        self.lookahead(1);
        /* Eat whitespaces. */
        while is_blank(self.ch()) {
            self.skip();
            self.lookahead(1);
        }
        let handle = self.scan_tag_handle(true, mark)?;

        self.lookahead(1);
        /* Eat whitespaces. */
        while is_blank(self.ch()) {
            self.skip();
            self.lookahead(1);
        }

        let is_secondary = handle == "!!";
        let prefix = self.scan_tag_uri(true, is_secondary, &String::new(), mark)?;

        self.lookahead(1);

        if is_blankz(self.ch()) {
            Ok(Token(*mark, TokenType::TagDirective(handle, prefix)))
        } else {
            Err(ScanError::new(
                *mark,
                "while scanning TAG, did not find expected whitespace or line break",
            ))
        }
    }

    fn fetch_tag(&mut self) -> ScanResult {
        self.save_simple_key()?;
        self.disallow_simple_key();

        let tok = self.scan_tag()?;
        self.tokens.push_back(tok);
        Ok(())
    }

    fn scan_tag(&mut self) -> Result<Token, ScanError> {
        let start_mark = self.mark;
        let mut handle = String::new();
        let mut suffix;
        let mut secondary = false;

        // Check if the tag is in the canonical form (verbatim).
        self.lookahead(2);

        if self.buffer[1] == '<' {
            // Eat '!<'
            self.skip();
            self.skip();
            suffix = self.scan_tag_uri(false, false, &String::new(), &start_mark)?;

            if self.ch() != '>' {
                return Err(ScanError::new(
                    start_mark,
                    "while scanning a tag, did not find the expected '>'",
                ));
            }

            self.skip();
        } else {
            // The tag has either the '!suffix' or the '!handle!suffix'
            handle = self.scan_tag_handle(false, &start_mark)?;
            // Check if it is, indeed, handle.
            if handle.len() >= 2 && handle.starts_with('!') && handle.ends_with('!') {
                if handle == "!!" {
                    secondary = true;
                }
                suffix = self.scan_tag_uri(false, secondary, &String::new(), &start_mark)?;
            } else {
                suffix = self.scan_tag_uri(false, false, &handle, &start_mark)?;
                handle = "!".to_owned();
                // A special case: the '!' tag.  Set the handle to '' and the
                // suffix to '!'.
                if suffix.is_empty() {
                    handle.clear();
                    suffix = "!".to_owned();
                }
            }
        }

        self.lookahead(1);
        if is_blankz(self.ch()) {
            // XXX: ex 7.2, an empty scalar can follow a secondary tag
            Ok(Token(start_mark, TokenType::Tag(handle, suffix)))
        } else {
            Err(ScanError::new(
                start_mark,
                "while scanning a tag, did not find expected whitespace or line break",
            ))
        }
    }

    fn scan_tag_handle(&mut self, directive: bool, mark: &Marker) -> Result<String, ScanError> {
        let mut string = String::new();
        self.lookahead(1);
        if self.ch() != '!' {
            return Err(ScanError::new(
                *mark,
                "while scanning a tag, did not find expected '!'",
            ));
        }

        string.push(self.ch());
        self.skip();

        self.lookahead(1);
        while is_alpha(self.ch()) {
            string.push(self.ch());
            self.skip();
            self.lookahead(1);
        }

        // Check if the trailing character is '!' and copy it.
        if self.ch() == '!' {
            string.push(self.ch());
            self.skip();
        } else if directive && string != "!" {
            // It's either the '!' tag or not really a tag handle.  If it's a %TAG
            // directive, it's an error.  If it's a tag token, it must be a part of
            // URI.
            return Err(ScanError::new(
                *mark,
                "while parsing a tag directive, did not find expected '!'",
            ));
        }
        Ok(string)
    }

    fn scan_tag_uri(
        &mut self,
        directive: bool,
        _is_secondary: bool,
        head: &str,
        mark: &Marker,
    ) -> Result<String, ScanError> {
        let mut length = head.len();
        let mut string = String::new();

        // Copy the head if needed.
        // Note that we don't copy the leading '!' character.
        if length > 1 {
            string.extend(head.chars().skip(1));
        }

        self.lookahead(1);
        /*
         * The set of characters that may appear in URI is as follows:
         *
         *      '0'-'9', 'A'-'Z', 'a'-'z', '_', '-', ';', '/', '?', ':', '@', '&',
         *      '=', '+', '$', ',', '.', '!', '~', '*', '\'', '(', ')', '[', ']',
         *      '%'.
         */
        while match self.ch() {
            ';' | '/' | '?' | ':' | '@' | '&' => true,
            '=' | '+' | '$' | ',' | '.' | '!' | '~' | '*' | '\'' | '(' | ')' | '[' | ']' => true,
            '%' => true,
            c if is_alpha(c) => true,
            _ => false,
        } {
            // Check if it is a URI-escape sequence.
            if self.ch() == '%' {
                string.push(self.scan_uri_escapes(directive, mark)?);
            } else {
                string.push(self.ch());
                self.skip();
            }

            length += 1;
            self.lookahead(1);
        }

        if length == 0 {
            return Err(ScanError::new(
                *mark,
                "while parsing a tag, did not find expected tag URI",
            ));
        }

        Ok(string)
    }

    fn scan_uri_escapes(&mut self, _directive: bool, mark: &Marker) -> Result<char, ScanError> {
        let mut width = 0usize;
        let mut code = 0u32;
        loop {
            self.lookahead(3);

            if !(self.ch() == '%' && is_hex(self.buffer[1]) && is_hex(self.buffer[2])) {
                return Err(ScanError::new(
                    *mark,
                    "while parsing a tag, did not find URI escaped octet",
                ));
            }

            let octet = (as_hex(self.buffer[1]) << 4) + as_hex(self.buffer[2]);
            if width == 0 {
                width = match octet {
                    _ if octet & 0x80 == 0x00 => 1,
                    _ if octet & 0xE0 == 0xC0 => 2,
                    _ if octet & 0xF0 == 0xE0 => 3,
                    _ if octet & 0xF8 == 0xF0 => 4,
                    _ => {
                        return Err(ScanError::new(
                            *mark,
                            "while parsing a tag, found an incorrect leading UTF-8 octet",
                        ));
                    }
                };
                code = octet;
            } else {
                if octet & 0xc0 != 0x80 {
                    return Err(ScanError::new(
                        *mark,
                        "while parsing a tag, found an incorrect trailing UTF-8 octet",
                    ));
                }
                code = (code << 8) + octet;
            }

            self.skip();
            self.skip();
            self.skip();

            width -= 1;
            if width == 0 {
                break;
            }
        }

        match char::from_u32(code) {
            Some(ch) => Ok(ch),
            None => Err(ScanError::new(
                *mark,
                "while parsing a tag, found an invalid UTF-8 codepoint",
            )),
        }
    }

    fn fetch_anchor(&mut self, alias: bool) -> ScanResult {
        self.save_simple_key()?;
        self.disallow_simple_key();

        let tok = self.scan_anchor(alias)?;

        self.tokens.push_back(tok);

        Ok(())
    }

    fn scan_anchor(&mut self, alias: bool) -> Result<Token, ScanError> {
        let mut string = String::new();
        let start_mark = self.mark;

        self.skip();
        self.lookahead(1);
        while is_alpha(self.ch()) {
            string.push(self.ch());
            self.skip();
            self.lookahead(1);
        }

        if string.is_empty()
            || match self.ch() {
                c if is_blankz(c) => false,
                '?' | ':' | ',' | ']' | '}' | '%' | '@' | '`' => false,
                _ => true,
            }
        {
            return Err(ScanError::new(start_mark, "while scanning an anchor or alias, did not find expected alphabetic or numeric character"));
        }

        if alias {
            Ok(Token(start_mark, TokenType::Alias(string)))
        } else {
            Ok(Token(start_mark, TokenType::Anchor(string)))
        }
    }

    fn fetch_flow_collection_start(&mut self, tok: TokenType) -> ScanResult {
        // The indicators '[' and '{' may start a simple key.
        self.save_simple_key()?;

        self.increase_flow_level()?;

        self.allow_simple_key();

        let start_mark = self.mark;
        self.skip();

        self.tokens.push_back(Token(start_mark, tok));
        Ok(())
    }

    fn fetch_flow_collection_end(&mut self, tok: TokenType) -> ScanResult {
        self.remove_simple_key()?;
        self.decrease_flow_level();

        self.disallow_simple_key();

        let start_mark = self.mark;
        self.skip();

        self.tokens.push_back(Token(start_mark, tok));
        Ok(())
    }

    fn fetch_flow_entry(&mut self) -> ScanResult {
        self.remove_simple_key()?;
        self.allow_simple_key();

        let start_mark = self.mark;
        self.skip();

        self.tokens
            .push_back(Token(start_mark, TokenType::FlowEntry));
        Ok(())
    }

    fn increase_flow_level(&mut self) -> ScanResult {
        self.simple_keys.push(SimpleKey::new(Marker::new(0, 0, 0)));
        self.flow_level = self
            .flow_level
            .checked_add(1)
            .ok_or_else(|| ScanError::new(self.mark, "recursion limit exceeded"))?;
        Ok(())
    }
    fn decrease_flow_level(&mut self) {
        if self.flow_level > 0 {
            self.flow_level -= 1;
            self.simple_keys.pop().unwrap();
        }
    }

    fn fetch_block_entry(&mut self) -> ScanResult {
        if self.flow_level == 0 {
            // Check if we are allowed to start a new entry.
            if !self.simple_key_allowed {
                return Err(ScanError::new(
                    self.mark,
                    "block sequence entries are not allowed in this context",
                ));
            }

            let mark = self.mark;
            // generate BLOCK-SEQUENCE-START if indented
            self.roll_indent(mark.col, None, TokenType::BlockSequenceStart, mark);
        } else {
            // - * only allowed in block
            return Err(ScanError::new(
                self.mark,
                r#""-" is only valid inside a block"#,
            ));
        }
        self.remove_simple_key()?;
        self.allow_simple_key();

        let start_mark = self.mark;
        self.skip();

        self.tokens
            .push_back(Token(start_mark, TokenType::BlockEntry));
        Ok(())
    }

    fn fetch_document_indicator(&mut self, t: TokenType) -> ScanResult {
        self.unroll_indent(-1);
        self.remove_simple_key()?;
        self.disallow_simple_key();

        let mark = self.mark;

        self.skip();
        self.skip();
        self.skip();

        self.tokens.push_back(Token(mark, t));
        Ok(())
    }

    fn fetch_block_scalar(&mut self, literal: bool) -> ScanResult {
        self.save_simple_key()?;
        self.allow_simple_key();
        let tok = self.scan_block_scalar(literal)?;

        self.tokens.push_back(tok);
        Ok(())
    }

    fn scan_block_scalar(&mut self, literal: bool) -> Result<Token, ScanError> {
        let start_mark = self.mark;
        let mut chomping: i32 = 0;
        let mut increment: usize = 0;
        let mut indent: usize = 0;
        let mut trailing_blank: bool;
        let mut leading_blank: bool = false;

        let mut string = String::new();
        let mut leading_break = String::new();
        let mut trailing_breaks = String::new();

        // skip '|' or '>'
        self.skip();
        self.lookahead(1);

        if self.ch() == '+' || self.ch() == '-' {
            if self.ch() == '+' {
                chomping = 1;
            } else {
                chomping = -1;
            }
            self.skip();
            self.lookahead(1);
            if is_digit(self.ch()) {
                if self.ch() == '0' {
                    return Err(ScanError::new(
                        start_mark,
                        "while scanning a block scalar, found an indentation indicator equal to 0",
                    ));
                }
                increment = (self.ch() as usize) - ('0' as usize);
                self.skip();
            }
        } else if is_digit(self.ch()) {
            if self.ch() == '0' {
                return Err(ScanError::new(
                    start_mark,
                    "while scanning a block scalar, found an indentation indicator equal to 0",
                ));
            }

            increment = (self.ch() as usize) - ('0' as usize);
            self.skip();
            self.lookahead(1);
            if self.ch() == '+' || self.ch() == '-' {
                if self.ch() == '+' {
                    chomping = 1;
                } else {
                    chomping = -1;
                }
                self.skip();
            }
        }

        // Eat whitespaces and comments to the end of the line.
        self.lookahead(1);

        while is_blank(self.ch()) {
            self.skip();
            self.lookahead(1);
        }

        if self.ch() == '#' {
            while !is_breakz(self.ch()) {
                self.skip();
                self.lookahead(1);
            }
        }

        // Check if we are at the end of the line.
        if !is_breakz(self.ch()) {
            return Err(ScanError::new(
                start_mark,
                "while scanning a block scalar, did not find expected comment or line break",
            ));
        }

        if is_break(self.ch()) {
            self.lookahead(2);
            self.skip_line();
        }

        if increment > 0 {
            indent = if self.indent >= 0 {
                (self.indent + increment as isize) as usize
            } else {
                increment
            }
        }
        // Scan the leading line breaks and determine the indentation level if needed.
        self.block_scalar_breaks(&mut indent, &mut trailing_breaks)?;

        self.lookahead(1);

        let start_mark = self.mark;

        while self.mark.col == indent && !is_z(self.ch()) {
            // We are at the beginning of a non-empty line.
            trailing_blank = is_blank(self.ch());
            if !literal && !leading_break.is_empty() && !leading_blank && !trailing_blank {
                if trailing_breaks.is_empty() {
                    string.push(' ');
                }
                leading_break.clear();
            } else {
                string.push_str(&leading_break);
                leading_break.clear();
            }

            string.push_str(&trailing_breaks);
            trailing_breaks.clear();

            leading_blank = is_blank(self.ch());

            while !is_breakz(self.ch()) {
                string.push(self.ch());
                self.skip();
                self.lookahead(1);
            }
            // break on EOF
            if is_z(self.ch()) {
                break;
            }

            self.lookahead(2);
            self.read_break(&mut leading_break);

            // Eat the following indentation spaces and line breaks.
            self.block_scalar_breaks(&mut indent, &mut trailing_breaks)?;
        }

        // Chomp the tail.
        if chomping != -1 {
            string.push_str(&leading_break);
        }

        if chomping == 1 {
            string.push_str(&trailing_breaks);
        }

        if literal {
            Ok(Token(
                start_mark,
                TokenType::Scalar(TScalarStyle::Literal, string),
            ))
        } else {
            Ok(Token(
                start_mark,
                TokenType::Scalar(TScalarStyle::Foled, string),
            ))
        }
    }

    fn block_scalar_breaks(&mut self, indent: &mut usize, breaks: &mut String) -> ScanResult {
        let mut max_indent = 0;
        loop {
            self.lookahead(1);
            while (*indent == 0 || self.mark.col < *indent) && self.buffer[0] == ' ' {
                self.skip();
                self.lookahead(1);
            }

            if self.mark.col > max_indent {
                max_indent = self.mark.col;
            }

            // Check for a tab character messing the indentation.
            if (*indent == 0 || self.mark.col < *indent) && self.buffer[0] == '\t' {
                return Err(ScanError::new(self.mark,
                        "while scanning a block scalar, found a tab character where an indentation space is expected"));
            }

            if !is_break(self.ch()) {
                break;
            }

            self.lookahead(2);
            // Consume the line break.
            self.read_break(breaks);
        }

        if *indent == 0 {
            *indent = max_indent;
            if *indent < (self.indent + 1) as usize {
                *indent = (self.indent + 1) as usize;
            }
            if *indent < 1 {
                *indent = 1;
            }
        }
        Ok(())
    }

    fn fetch_flow_scalar(&mut self, single: bool) -> ScanResult {
        self.save_simple_key()?;
        self.disallow_simple_key();

        let tok = self.scan_flow_scalar(single)?;

        // From spec: To ensure JSON compatibility, if a key inside a flow mapping is JSON-like,
        // YAML allows the following value to be specified adjacent to the “:”.
        self.adjacent_value_allowed_at = self.mark.index;

        self.tokens.push_back(tok);
        Ok(())
    }

    fn scan_flow_scalar(&mut self, single: bool) -> Result<Token, ScanError> {
        let start_mark = self.mark;

        let mut string = String::new();
        let mut leading_break = String::new();
        let mut trailing_breaks = String::new();
        let mut whitespaces = String::new();
        let mut leading_blanks;

        /* Eat the left quote. */
        self.skip();

        loop {
            /* Check for a document indicator. */
            self.lookahead(4);

            if self.mark.col == 0
                && (((self.buffer[0] == '-') && (self.buffer[1] == '-') && (self.buffer[2] == '-'))
                    || ((self.buffer[0] == '.')
                        && (self.buffer[1] == '.')
                        && (self.buffer[2] == '.')))
                && is_blankz(self.buffer[3])
            {
                return Err(ScanError::new(
                    start_mark,
                    "while scanning a quoted scalar, found unexpected document indicator",
                ));
            }

            if is_z(self.ch()) {
                return Err(ScanError::new(
                    start_mark,
                    "while scanning a quoted scalar, found unexpected end of stream",
                ));
            }

            self.lookahead(2);

            leading_blanks = false;
            // Consume non-blank characters.

            while !is_blankz(self.ch()) {
                match self.ch() {
                    // Check for an escaped single quote.
                    '\'' if self.buffer[1] == '\'' && single => {
                        string.push('\'');
                        self.skip();
                        self.skip();
                    }
                    // Check for the right quote.
                    '\'' if single => break,
                    '"' if !single => break,
                    // Check for an escaped line break.
                    '\\' if !single && is_break(self.buffer[1]) => {
                        self.lookahead(3);
                        self.skip();
                        self.skip_line();
                        leading_blanks = true;
                        break;
                    }
                    // Check for an escape sequence.
                    '\\' if !single => {
                        let mut code_length = 0usize;
                        match self.buffer[1] {
                            '0' => string.push('\0'),
                            'a' => string.push('\x07'),
                            'b' => string.push('\x08'),
                            't' | '\t' => string.push('\t'),
                            'n' => string.push('\n'),
                            'v' => string.push('\x0b'),
                            'f' => string.push('\x0c'),
                            'r' => string.push('\x0d'),
                            'e' => string.push('\x1b'),
                            ' ' => string.push('\x20'),
                            '"' => string.push('"'),
                            '\'' => string.push('\''),
                            '\\' => string.push('\\'),
                            // NEL (#x85)
                            'N' => string.push(char::from_u32(0x85).unwrap()),
                            // #xA0
                            '_' => string.push(char::from_u32(0xA0).unwrap()),
                            // LS (#x2028)
                            'L' => string.push(char::from_u32(0x2028).unwrap()),
                            // PS (#x2029)
                            'P' => string.push(char::from_u32(0x2029).unwrap()),
                            'x' => code_length = 2,
                            'u' => code_length = 4,
                            'U' => code_length = 8,
                            _ => {
                                return Err(ScanError::new(
                                    start_mark,
                                    "while parsing a quoted scalar, found unknown escape character",
                                ))
                            }
                        }
                        self.skip();
                        self.skip();
                        // Consume an arbitrary escape code.
                        if code_length > 0 {
                            self.lookahead(code_length);
                            let mut value = 0u32;
                            for i in 0..code_length {
                                if !is_hex(self.buffer[i]) {
                                    return Err(ScanError::new(start_mark,
                                        "while parsing a quoted scalar, did not find expected hexadecimal number"));
                                }
                                value = (value << 4) + as_hex(self.buffer[i]);
                            }

                            let ch = match char::from_u32(value) {
                                Some(v) => v,
                                None => {
                                    return Err(ScanError::new(start_mark,
                                        "while parsing a quoted scalar, found invalid Unicode character escape code"));
                                }
                            };
                            string.push(ch);

                            for _ in 0..code_length {
                                self.skip();
                            }
                        }
                    }
                    c => {
                        string.push(c);
                        self.skip();
                    }
                }
                self.lookahead(2);
            }
            self.lookahead(1);
            match self.ch() {
                '\'' if single => break,
                '"' if !single => break,
                _ => {}
            }

            // Consume blank characters.
            while is_blank(self.ch()) || is_break(self.ch()) {
                if is_blank(self.ch()) {
                    // Consume a space or a tab character.
                    if leading_blanks {
                        self.skip();
                    } else {
                        whitespaces.push(self.ch());
                        self.skip();
                    }
                } else {
                    self.lookahead(2);
                    // Check if it is a first line break.
                    if leading_blanks {
                        self.read_break(&mut trailing_breaks);
                    } else {
                        whitespaces.clear();
                        self.read_break(&mut leading_break);
                        leading_blanks = true;
                    }
                }
                self.lookahead(1);
            }
            // Join the whitespaces or fold line breaks.
            if leading_blanks {
                if leading_break.is_empty() {
                    string.push_str(&leading_break);
                    string.push_str(&trailing_breaks);
                    trailing_breaks.clear();
                    leading_break.clear();
                } else {
                    if trailing_breaks.is_empty() {
                        string.push(' ');
                    } else {
                        string.push_str(&trailing_breaks);
                        trailing_breaks.clear();
                    }
                    leading_break.clear();
                }
            } else {
                string.push_str(&whitespaces);
                whitespaces.clear();
            }
        } // loop

        // Eat the right quote.
        self.skip();

        if single {
            Ok(Token(
                start_mark,
                TokenType::Scalar(TScalarStyle::SingleQuoted, string),
            ))
        } else {
            Ok(Token(
                start_mark,
                TokenType::Scalar(TScalarStyle::DoubleQuoted, string),
            ))
        }
    }

    fn fetch_plain_scalar(&mut self) -> ScanResult {
        self.save_simple_key()?;
        self.disallow_simple_key();

        let tok = self.scan_plain_scalar()?;

        self.tokens.push_back(tok);
        Ok(())
    }

    fn scan_plain_scalar(&mut self) -> Result<Token, ScanError> {
        let indent = self.indent + 1;
        let start_mark = self.mark;

        let mut string = String::new();
        let mut leading_break = String::new();
        let mut trailing_breaks = String::new();
        let mut whitespaces = String::new();
        let mut leading_blanks = false;

        loop {
            /* Check for a document indicator. */
            self.lookahead(4);

            if self.mark.col == 0
                && (((self.buffer[0] == '-') && (self.buffer[1] == '-') && (self.buffer[2] == '-'))
                    || ((self.buffer[0] == '.')
                        && (self.buffer[1] == '.')
                        && (self.buffer[2] == '.')))
                && is_blankz(self.buffer[3])
            {
                break;
            }

            if self.ch() == '#' {
                break;
            }
            while !is_blankz(self.ch()) {
                // indicators can end a plain scalar, see 7.3.3. Plain Style
                match self.ch() {
                    ':' if is_blankz(self.buffer[1])
                        || (self.flow_level > 0 && is_flow(self.buffer[1])) =>
                    {
                        break;
                    }
                    ',' | '[' | ']' | '{' | '}' if self.flow_level > 0 => break,
                    _ => {}
                }

                if leading_blanks || !whitespaces.is_empty() {
                    if leading_blanks {
                        if leading_break.is_empty() {
                            string.push_str(&leading_break);
                            string.push_str(&trailing_breaks);
                            trailing_breaks.clear();
                            leading_break.clear();
                        } else {
                            if trailing_breaks.is_empty() {
                                string.push(' ');
                            } else {
                                string.push_str(&trailing_breaks);
                                trailing_breaks.clear();
                            }
                            leading_break.clear();
                        }
                        leading_blanks = false;
                    } else {
                        string.push_str(&whitespaces);
                        whitespaces.clear();
                    }
                }

                string.push(self.ch());
                self.skip();
                self.lookahead(2);
            }
            // is the end?
            if !(is_blank(self.ch()) || is_break(self.ch())) {
                break;
            }
            self.lookahead(1);

            while is_blank(self.ch()) || is_break(self.ch()) {
                if is_blank(self.ch()) {
                    if leading_blanks && (self.mark.col as isize) < indent && self.ch() == '\t' {
                        return Err(ScanError::new(
                            start_mark,
                            "while scanning a plain scalar, found a tab",
                        ));
                    }

                    if leading_blanks {
                        self.skip();
                    } else {
                        whitespaces.push(self.ch());
                        self.skip();
                    }
                } else {
                    self.lookahead(2);
                    // Check if it is a first line break
                    if leading_blanks {
                        self.read_break(&mut trailing_breaks);
                    } else {
                        whitespaces.clear();
                        self.read_break(&mut leading_break);
                        leading_blanks = true;
                    }
                }
                self.lookahead(1);
            }

            // check indentation level
            if self.flow_level == 0 && (self.mark.col as isize) < indent {
                break;
            }
        }

        if leading_blanks {
            self.allow_simple_key();
        }

        Ok(Token(
            start_mark,
            TokenType::Scalar(TScalarStyle::Plain, string),
        ))
    }

    fn fetch_key(&mut self) -> ScanResult {
        let start_mark = self.mark;
        if self.flow_level == 0 {
            // Check if we are allowed to start a new key (not necessarily simple).
            if !self.simple_key_allowed {
                return Err(ScanError::new(
                    self.mark,
                    "mapping keys are not allowed in this context",
                ));
            }
            self.roll_indent(
                start_mark.col,
                None,
                TokenType::BlockMappingStart,
                start_mark,
            );
        }

        self.remove_simple_key()?;

        if self.flow_level == 0 {
            self.allow_simple_key();
        } else {
            self.disallow_simple_key();
        }

        self.skip();
        self.tokens.push_back(Token(start_mark, TokenType::Key));
        Ok(())
    }

    fn fetch_value(&mut self) -> ScanResult {
        let sk = self.simple_keys.last().unwrap().clone();
        let start_mark = self.mark;
        if sk.possible {
            // insert simple key
            let tok = Token(sk.mark, TokenType::Key);
            let tokens_parsed = self.tokens_parsed;
            self.insert_token(sk.token_number - tokens_parsed, tok);

            // Add the BLOCK-MAPPING-START token if needed.
            self.roll_indent(
                sk.mark.col,
                Some(sk.token_number),
                TokenType::BlockMappingStart,
                start_mark,
            );

            self.simple_keys.last_mut().unwrap().possible = false;
            self.disallow_simple_key();
        } else {
            // The ':' indicator follows a complex key.
            if self.flow_level == 0 {
                if !self.simple_key_allowed {
                    return Err(ScanError::new(
                        start_mark,
                        "mapping values are not allowed in this context",
                    ));
                }

                self.roll_indent(
                    start_mark.col,
                    None,
                    TokenType::BlockMappingStart,
                    start_mark,
                );
            }

            if self.flow_level == 0 {
                self.allow_simple_key();
            } else {
                self.disallow_simple_key();
            }
        }
        self.skip();
        self.tokens.push_back(Token(start_mark, TokenType::Value));

        Ok(())
    }

    fn roll_indent(&mut self, col: usize, number: Option<usize>, tok: TokenType, mark: Marker) {
        if self.flow_level > 0 {
            return;
        }

        if self.indent < col as isize {
            self.indents.push(self.indent);
            self.indent = col as isize;
            let tokens_parsed = self.tokens_parsed;
            match number {
                Some(n) => self.insert_token(n - tokens_parsed, Token(mark, tok)),
                None => self.tokens.push_back(Token(mark, tok)),
            }
        }
    }

    fn unroll_indent(&mut self, col: isize) {
        if self.flow_level > 0 {
            return;
        }
        while self.indent > col {
            self.tokens.push_back(Token(self.mark, TokenType::BlockEnd));
            self.indent = self.indents.pop().unwrap();
        }
    }

    fn save_simple_key(&mut self) -> Result<(), ScanError> {
        let required = self.flow_level > 0 && self.indent == (self.mark.col as isize);
        if self.simple_key_allowed {
            let mut sk = SimpleKey::new(self.mark);
            sk.possible = true;
            sk.required = required;
            sk.token_number = self.tokens_parsed + self.tokens.len();

            self.remove_simple_key()?;

            self.simple_keys.pop();
            self.simple_keys.push(sk);
        }
        Ok(())
    }

    fn remove_simple_key(&mut self) -> ScanResult {
        let last = self.simple_keys.last_mut().unwrap();
        if last.possible && last.required {
            return Err(ScanError::new(self.mark, "simple key expected"));
        }

        last.possible = false;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::TokenType::*;
    use super::*;

    macro_rules! next {
        ($p:ident, $tk:pat) => {{
            let tok = $p.next().unwrap();
            match tok.1 {
                $tk => {}
                _ => panic!("unexpected token: {:?}", tok),
            }
        }};
    }

    macro_rules! next_scalar {
        ($p:ident, $tk:expr, $v:expr) => {{
            let tok = $p.next().unwrap();
            match tok.1 {
                Scalar(style, ref v) => {
                    assert_eq!(style, $tk);
                    assert_eq!(v, $v);
                }
                _ => panic!("unexpected token: {:?}", tok),
            }
        }};
    }

    macro_rules! end {
        ($p:ident) => {{
            assert_eq!($p.next(), None);
        }};
    }
    /// test cases in libyaml scanner.c
    #[test]
    fn test_empty() {
        let s = "";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_scalar() {
        let s = "a scalar";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, Scalar(TScalarStyle::Plain, _));
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_explicit_scalar() {
        let s = "---
'a scalar'
...
";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, DocumentStart);
        next!(p, Scalar(TScalarStyle::SingleQuoted, _));
        next!(p, DocumentEnd);
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_multiple_documents() {
        let s = "
'a scalar'
---
'a scalar'
---
'a scalar'
";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, Scalar(TScalarStyle::SingleQuoted, _));
        next!(p, DocumentStart);
        next!(p, Scalar(TScalarStyle::SingleQuoted, _));
        next!(p, DocumentStart);
        next!(p, Scalar(TScalarStyle::SingleQuoted, _));
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_a_flow_sequence() {
        let s = "[item 1, item 2, item 3]";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, FlowSequenceStart);
        next_scalar!(p, TScalarStyle::Plain, "item 1");
        next!(p, FlowEntry);
        next!(p, Scalar(TScalarStyle::Plain, _));
        next!(p, FlowEntry);
        next!(p, Scalar(TScalarStyle::Plain, _));
        next!(p, FlowSequenceEnd);
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_a_flow_mapping() {
        let s = "
{
    a simple key: a value, # Note that the KEY token is produced.
    ? a complex key: another value,
}
";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, FlowMappingStart);
        next!(p, Key);
        next!(p, Scalar(TScalarStyle::Plain, _));
        next!(p, Value);
        next!(p, Scalar(TScalarStyle::Plain, _));
        next!(p, FlowEntry);
        next!(p, Key);
        next_scalar!(p, TScalarStyle::Plain, "a complex key");
        next!(p, Value);
        next!(p, Scalar(TScalarStyle::Plain, _));
        next!(p, FlowEntry);
        next!(p, FlowMappingEnd);
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_block_sequences() {
        let s = "
- item 1
- item 2
-
  - item 3.1
  - item 3.2
-
  key 1: value 1
  key 2: value 2
";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, BlockSequenceStart);
        next!(p, BlockEntry);
        next_scalar!(p, TScalarStyle::Plain, "item 1");
        next!(p, BlockEntry);
        next_scalar!(p, TScalarStyle::Plain, "item 2");
        next!(p, BlockEntry);
        next!(p, BlockSequenceStart);
        next!(p, BlockEntry);
        next_scalar!(p, TScalarStyle::Plain, "item 3.1");
        next!(p, BlockEntry);
        next_scalar!(p, TScalarStyle::Plain, "item 3.2");
        next!(p, BlockEnd);
        next!(p, BlockEntry);
        next!(p, BlockMappingStart);
        next!(p, Key);
        next_scalar!(p, TScalarStyle::Plain, "key 1");
        next!(p, Value);
        next_scalar!(p, TScalarStyle::Plain, "value 1");
        next!(p, Key);
        next_scalar!(p, TScalarStyle::Plain, "key 2");
        next!(p, Value);
        next_scalar!(p, TScalarStyle::Plain, "value 2");
        next!(p, BlockEnd);
        next!(p, BlockEnd);
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_block_mappings() {
        let s = "
a simple key: a value   # The KEY token is produced here.
? a complex key
: another value
a mapping:
  key 1: value 1
  key 2: value 2
a sequence:
  - item 1
  - item 2
";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, BlockMappingStart);
        next!(p, Key);
        next!(p, Scalar(_, _));
        next!(p, Value);
        next!(p, Scalar(_, _));
        next!(p, Key);
        next!(p, Scalar(_, _));
        next!(p, Value);
        next!(p, Scalar(_, _));
        next!(p, Key);
        next!(p, Scalar(_, _));
        next!(p, Value); // libyaml comment seems to be wrong
        next!(p, BlockMappingStart);
        next!(p, Key);
        next!(p, Scalar(_, _));
        next!(p, Value);
        next!(p, Scalar(_, _));
        next!(p, Key);
        next!(p, Scalar(_, _));
        next!(p, Value);
        next!(p, Scalar(_, _));
        next!(p, BlockEnd);
        next!(p, Key);
        next!(p, Scalar(_, _));
        next!(p, Value);
        next!(p, BlockSequenceStart);
        next!(p, BlockEntry);
        next!(p, Scalar(_, _));
        next!(p, BlockEntry);
        next!(p, Scalar(_, _));
        next!(p, BlockEnd);
        next!(p, BlockEnd);
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_no_block_sequence_start() {
        let s = "
key:
- item 1
- item 2
";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, BlockMappingStart);
        next!(p, Key);
        next_scalar!(p, TScalarStyle::Plain, "key");
        next!(p, Value);
        next!(p, BlockEntry);
        next_scalar!(p, TScalarStyle::Plain, "item 1");
        next!(p, BlockEntry);
        next_scalar!(p, TScalarStyle::Plain, "item 2");
        next!(p, BlockEnd);
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_collections_in_sequence() {
        let s = "
- - item 1
  - item 2
- key 1: value 1
  key 2: value 2
- ? complex key
  : complex value
";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, BlockSequenceStart);
        next!(p, BlockEntry);
        next!(p, BlockSequenceStart);
        next!(p, BlockEntry);
        next_scalar!(p, TScalarStyle::Plain, "item 1");
        next!(p, BlockEntry);
        next_scalar!(p, TScalarStyle::Plain, "item 2");
        next!(p, BlockEnd);
        next!(p, BlockEntry);
        next!(p, BlockMappingStart);
        next!(p, Key);
        next_scalar!(p, TScalarStyle::Plain, "key 1");
        next!(p, Value);
        next_scalar!(p, TScalarStyle::Plain, "value 1");
        next!(p, Key);
        next_scalar!(p, TScalarStyle::Plain, "key 2");
        next!(p, Value);
        next_scalar!(p, TScalarStyle::Plain, "value 2");
        next!(p, BlockEnd);
        next!(p, BlockEntry);
        next!(p, BlockMappingStart);
        next!(p, Key);
        next_scalar!(p, TScalarStyle::Plain, "complex key");
        next!(p, Value);
        next_scalar!(p, TScalarStyle::Plain, "complex value");
        next!(p, BlockEnd);
        next!(p, BlockEnd);
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_collections_in_mapping() {
        let s = "
? a sequence
: - item 1
  - item 2
? a mapping
: key 1: value 1
  key 2: value 2
";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, BlockMappingStart);
        next!(p, Key);
        next_scalar!(p, TScalarStyle::Plain, "a sequence");
        next!(p, Value);
        next!(p, BlockSequenceStart);
        next!(p, BlockEntry);
        next_scalar!(p, TScalarStyle::Plain, "item 1");
        next!(p, BlockEntry);
        next_scalar!(p, TScalarStyle::Plain, "item 2");
        next!(p, BlockEnd);
        next!(p, Key);
        next_scalar!(p, TScalarStyle::Plain, "a mapping");
        next!(p, Value);
        next!(p, BlockMappingStart);
        next!(p, Key);
        next_scalar!(p, TScalarStyle::Plain, "key 1");
        next!(p, Value);
        next_scalar!(p, TScalarStyle::Plain, "value 1");
        next!(p, Key);
        next_scalar!(p, TScalarStyle::Plain, "key 2");
        next!(p, Value);
        next_scalar!(p, TScalarStyle::Plain, "value 2");
        next!(p, BlockEnd);
        next!(p, BlockEnd);
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_spec_ex7_3() {
        let s = "
{
    ? foo :,
    : bar,
}
";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, FlowMappingStart);
        next!(p, Key);
        next_scalar!(p, TScalarStyle::Plain, "foo");
        next!(p, Value);
        next!(p, FlowEntry);
        next!(p, Value);
        next_scalar!(p, TScalarStyle::Plain, "bar");
        next!(p, FlowEntry);
        next!(p, FlowMappingEnd);
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_plain_scalar_starting_with_indicators_in_flow() {
        // "Plain scalars must not begin with most indicators, as this would cause ambiguity with
        // other YAML constructs. However, the “:”, “?” and “-” indicators may be used as the first
        // character if followed by a non-space “safe” character, as this causes no ambiguity."

        let s = "{a: :b}";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, FlowMappingStart);
        next!(p, Key);
        next_scalar!(p, TScalarStyle::Plain, "a");
        next!(p, Value);
        next_scalar!(p, TScalarStyle::Plain, ":b");
        next!(p, FlowMappingEnd);
        next!(p, StreamEnd);
        end!(p);

        let s = "{a: ?b}";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, FlowMappingStart);
        next!(p, Key);
        next_scalar!(p, TScalarStyle::Plain, "a");
        next!(p, Value);
        next_scalar!(p, TScalarStyle::Plain, "?b");
        next!(p, FlowMappingEnd);
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_plain_scalar_starting_with_indicators_in_block() {
        let s = ":a";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next_scalar!(p, TScalarStyle::Plain, ":a");
        next!(p, StreamEnd);
        end!(p);

        let s = "?a";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next_scalar!(p, TScalarStyle::Plain, "?a");
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_plain_scalar_containing_indicators_in_block() {
        let s = "a:,b";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next_scalar!(p, TScalarStyle::Plain, "a:,b");
        next!(p, StreamEnd);
        end!(p);

        let s = ":,b";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next_scalar!(p, TScalarStyle::Plain, ":,b");
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_scanner_cr() {
        let s = "---\r\n- tok1\r\n- tok2";
        let mut p = Scanner::new(s.chars());
        next!(p, StreamStart(..));
        next!(p, DocumentStart);
        next!(p, BlockSequenceStart);
        next!(p, BlockEntry);
        next_scalar!(p, TScalarStyle::Plain, "tok1");
        next!(p, BlockEntry);
        next_scalar!(p, TScalarStyle::Plain, "tok2");
        next!(p, BlockEnd);
        next!(p, StreamEnd);
        end!(p);
    }

    #[test]
    fn test_uri() {
        // TODO
    }

    #[test]
    fn test_uri_escapes() {
        // TODO
    }
}
