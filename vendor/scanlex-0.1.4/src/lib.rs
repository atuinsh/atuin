//! `scanlex` implements a simple _lexical scanner_.
//!
//! Tokens are returned by repeatedly calling the `get` method,
//! (which will return `Token::End` if no tokens are left)
//! or by iterating over the scanner.
//!
//! They represent floats (stored as f64), integers (as i64), characters, identifiers,
//! or single or double quoted strings. There is also `Token::Error` to
//! indicate a badly formed token.  This lexical scanner makes some
//! sensible assumptions, such as a number may not be directly followed
//! by a letter, etc. No attempt is made in this version to decode C-style
//! escape codes in strings.  All whitespace is ignored.
//!
//! ## Examples
//!
//! ```
//! use  scanlex::{Scanner,Token};
//!
//! let mut scan = Scanner::new("iden 'string' * 10");
//! assert_eq!(scan.get(),Token::Iden("iden".into()));
//! assert_eq!(scan.get(),Token::Str("string".into()));
//! assert_eq!(scan.get(),Token::Char('*'));
//! assert_eq!(scan.get(),Token::Int(10));
//! assert_eq!(scan.get(),Token::End);
//! ```
//!
//! The scanner struct implements iterator, so:
//!
//! ```
//! let v: Vec<_> = scanlex::Scanner::new("bonzo 42 dog (cat)")
//!     .filter_map(|t| t.to_iden()).collect();
//! assert_eq!(v,&["bonzo","dog","cat"]);
//! ```

use std::str::FromStr;
use std::error::Error;
use std::io;

mod int;
use int::Int;

mod error;
pub use error::ScanError;

mod token;
pub use token::Token;

/// a struct for lexical scanning of a string
pub struct Scanner <'a> {
    iter: ::std::str::Chars<'a>,
    ch: char,
    pub lineno: u32,
    no_float: bool,
    line_comment: Option<char>,
}

fn expecting_chars(chars: &[char]) -> String {
    let mut res = String::new();
    for c in chars {
        res.push_str(&format!("'{}'",c));
        res.push(',')
    }
    res.pop();
    res
}

impl<'a> Iterator for Scanner<'a> {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        match self.get() {
            Token::End => None,
            t => Some(t)
        }
    }
}

impl<'a> Scanner<'a> {
    /// create a new scanner from a string slice.
    ///
    /// Empty text is not a problem, but `get` will then
    /// return `Token::End`.
    pub fn new(s: &'a str) -> Scanner<'a> {
        Scanner::new_ex(s,1)
    }

    fn new_ex(s: &'a str, lineno: u32) -> Scanner<'a> {
        let mut iter = s.chars();
        let mch = iter.next();
        Scanner {
            iter: iter,
            ch: match mch {Some(c) => c, None => '\0'},
            lineno: lineno,
            no_float: false,
            line_comment: None,
        }
    }

    /// this scanner will not recognize floats
    ///
    /// "2.5" is tokenized as Int(2),Char('.'),Int(5)
    pub fn no_float(mut self) -> Scanner<'a> {
        self.no_float = true;
        self
    }

    /// ignore everything in a line after this char
    pub fn line_comment(mut self, c: char) -> Scanner<'a> {
        self.line_comment = Some(c);
        self
    }
    

    pub fn scan_error(&self, msg: &str, cause: Option<&dyn Error>) -> ScanError {
       ScanError{
           details: format!("{}{}", msg,
                match cause {
                    Some(c) => format!(": caused by {}",c),
                    None => "".into()
                }
            ),
            lineno: self.lineno
        }
    }

    fn update_lineno(&self, mut err: ScanError) -> ScanError {
        err.lineno = self.lineno;
        err
    }

    fn token_error(&self, msg: &str, cause: Option<&dyn Error>) -> Token {
        Token::Error(self.scan_error(msg,cause))
    }

    fn check_line_comment(&mut self) -> bool {
        if let Some(lc) = self.line_comment {
            if self.ch == lc {
                self.skip_until(|c| c=='\n');
                return true;
            }
        }
        return false;

    }

    /// skip any whitespace characters - return false if we're at the end.
    pub fn skip_whitespace(&mut self) -> bool {
        loop {
            self.check_line_comment();
            if self.ch.is_whitespace() {
                if self.ch == '\n' {
                     self.lineno += 1;
                }
                while let Some(c) = self.iter.next() {
                    if c == '\n' {
                        self.lineno += 1;
                    }
                    if ! c.is_whitespace() {
                        self.ch = c;
                        if self.check_line_comment() {
                            continue;
                        } else {
                            return true;
                        }
                    }
                }
                // run of chars!
                self.ch = '\0';
                break;
            } else {
                break;
            }
        }
        if self.ch == '\0' {
            false
        } else {
            true
        }
    }

    /// look ahead at the next character
    pub fn peek(&self) -> char {
        self.ch
    }

    /// get the next character
    pub fn nextch(&mut self) -> char {
        let old_ch = self.ch;
        self.ch = match self.iter.next() {
            Some(c) => c,
            None => '\0'
        };
        old_ch
    }

    fn either_plus_or_minus(&self) -> Option<char> {
        if self.ch == '+' || self.ch == '-' {
            Some(self.ch)
        } else {
            None
        }
    }

    fn is_digit(&self) -> bool {
        self.ch.is_digit(10)
    }

    /// get the next token
    pub fn get(&mut self) -> Token {
        use self::Token::*;
        if ! self.skip_whitespace() {
            return End;
        }

        // a number starts with a digit or a sign
        let plusminus = if ! self.no_float {self.either_plus_or_minus()} else {None};
        if self.is_digit() || plusminus.is_some() {
            let mut s = String::new();
            if plusminus.is_some() {
                s.push(plusminus.unwrap());
            }
            if ! self.no_float {
                let mut maybe_hex = self.ch == '0';
                if plusminus.is_some() || maybe_hex {
                    // look ahead! Might be a number or just a char
                    self.nextch();
                    if maybe_hex { // after a '0'?
                        maybe_hex = self.ch == 'X' || self.ch == 'x';
                        if ! maybe_hex {
                            s.push('0');
                            if ! self.is_digit() && self.ch != '.' { self.ch = '\0'; }
                        }
                    } else
                    if ! self.is_digit() { // false alarm, wuz just a char...
                        return Char(plusminus.unwrap());
                    }
                }
                // integer part
                if maybe_hex { // in hex...
                    self.nextch(); // skip the 'x'
                    self.take_while_into(&mut s,|c| c.is_digit(16));
                    return match i64::from_str_radix(&s,16) {
                        Ok(n) => Int(n),
                        Err(e) => self.token_error("bad hex constant",Some(&e))
                    }
                }
            }

            if self.ch != '.' { // for 0. case - we already peeked ahead
                self.take_digits_into(&mut s);
            }

            // floating point part?
            if ! self.no_float && (self.ch == '.'  || self.ch == 'e' || self.ch == 'E') {
                if self.ch == '.' {
                    self.take_digits_into(&mut s);
                }
                if self.ch == 'e' || self.ch == 'E' {
                    s.push(self.nextch());
                    if self.is_digit() || self.either_plus_or_minus().is_some() {
                        self.take_digits_into(&mut s);
                    }
                }
                return if self.ch.is_alphabetic() {
                    self.token_error("bad floating-point number: letter follows",None)
                } else {
                    match f64::from_str(&s) {
                        Ok(x) => Num(x),
                        Err(e) => self.token_error(&format!("bad floating-point number {:?}",s),Some(&e))
                    }
                }
            } else {
                return if ! self.no_float && self.ch.is_alphabetic() {
                    self.token_error("bad integer: letter follows",None)
                } else {
                    match i64::from_str(&s) {
                        Ok(x) => Int(x),
                        Err(e) => self.token_error(&format!("bad integer {:?}",s),Some(&e))
                    }
                }
            }
        } else
        if self.ch == '\'' || self.ch == '\"' {
            let endquote = self.ch;
            self.nextch(); // skip the opening quote
            let s = self.grab_while(|c| c != endquote);
            // TODO unfinished quote
            self.nextch();  // skip end quote
            Str(s)
        } else
        if self.ch.is_alphabetic() || self.ch == '_' {
            let s = self.grab_while(|c| c.is_alphanumeric() || c == '_');
            Iden(s)
        } else {
            Char(self.nextch())
        }
    }

    /// collect chars matching the condition, returning a string
    /// ```
    /// let mut scan = scanlex::Scanner::new("hello + goodbye");
    /// assert_eq!(scan.grab_while(|c| c != '+'), "hello ");
    /// ```
    pub fn grab_while<F>(&mut self, pred: F ) -> String
     where F: Fn(char) -> bool {
        let mut s = String::new();
        self.take_while_into(&mut s,pred);
        s
    }

    /// collect chars matching the condition into a given string
    pub fn take_while_into<F>(&mut self, s: &mut String, pred: F )
     where F: Fn(char) -> bool {
        if self.ch != '\0' {
            s.push(self.ch);
        }
        while let Some(c) = self.iter.next() {
            if ! pred(c) { self.ch = c; return; }
            s.push(c);
        }
        self.ch = '\0';
    }

    fn take_digits_into(&mut self, s: &mut String) {
        self.take_while_into(s, |c| c.is_digit(10));
    }

    /// skip chars while the condition is false
    ///
    /// ```
    /// let mut scan = scanlex::Scanner::new("hello and\nwelcome");
    /// scan.skip_until(|c| c == '\n');
    /// assert_eq!(scan.get_iden().unwrap(),"welcome");
    /// ```
    pub fn skip_until<F>(&mut self, pred: F ) -> bool
    where F: Fn(char) -> bool {
        while let Some(c) = self.iter.next() {
            if pred(c) { self.ch = c; return true; }
        }
        self.ch = '\0';
        false
    }

    /// collect the rest of the chars
    ///
    /// ```
    /// use scanlex::{Scanner,Token};
    ///
    /// let mut scan = Scanner::new("42 the answer");
    /// assert_eq!(scan.get(),Token::Int(42));
    /// assert_eq!(scan.take_rest()," the answer");
    /// ```
    pub fn take_rest(&mut self) -> String {
        self.grab_while(|c| c != '\0')
    }

    /// collect until we match one of the chars
    pub fn take_until (&mut self, chars: &[char]) -> String {
        self.grab_while(|c| ! chars.contains(&c))
    }

    /// get a String token, failing otherwise
    pub fn get_string(&mut self) -> Result<String,ScanError> {
        self.get().to_string_result().map_err(|e| self.update_lineno(e))
    }

    /// get an Identifier token, failing otherwise
    ///
    /// ```
    /// let mut scan = scanlex::Scanner::new("hello dolly");
    /// assert_eq!(scan.get_iden().unwrap(),"hello");
    /// ```
    pub fn get_iden(&mut self) -> Result<String,ScanError> {
        self.get().to_iden_result().map_err(|e| self.update_lineno(e))
    }

    /// get a number, failing otherwise
    ///
    /// ```
    /// let mut scan = scanlex::Scanner::new("(42)");
    /// scan.get(); // skip '('
    /// assert_eq!(scan.get_number().unwrap(),42.0);
    /// ```
    pub fn get_number(&mut self) -> Result<f64,ScanError> {
        self.get().to_number_result().map_err(|e| self.update_lineno(e))
    }

    /// get an integer, failing otherwise
    pub fn get_integer(&mut self) -> Result<i64,ScanError> {
        self.get().to_integer_result().map_err(|e| self.update_lineno(e))
    }

    /// get an integer of a particular type, failing otherwise
    pub fn get_int<I: Int>(&mut self) -> Result<I::Type,ScanError> {
        self.get().to_int_result::<I>().map_err(|e| self.update_lineno(e))
    }

    /// get an float, failing otherwise
    pub fn get_float(&mut self) -> Result<f64,ScanError> {
        self.get().to_float_result().map_err(|e| self.update_lineno(e))
    }

    /// get a character, failing otherwise
    pub fn get_char(&mut self) -> Result<char,ScanError> {
        self.get().to_char_result().map_err(|e| self.update_lineno(e))
    }

    /// get a Character token that must be one of the given chars
    pub fn get_ch_matching(&mut self, chars: &[char]) -> Result<char,ScanError> {
        let c = self.get_char()?;
        if chars.contains(&c) {
            Ok(c)
        } else {
            let s = expecting_chars(chars);
            Err(self.scan_error(&format!("expected one of {}, got {}",s,c),None))
        }
    }

    /// skip each character in the string.
    pub fn skip_chars(&mut self, chars: &str) -> Result<(),ScanError> {
        for ch in chars.chars() {
            let c = self.get_char()?;
            if c != ch {
                return Err(self.scan_error(&format!("expected '{}' got '{}'",ch,c),None));
            }
        }
        Ok(())
    }

    /// grab 'balanced' text between some open and close chars
    pub fn grab_brackets(&mut self, pair: &str) -> Result<String,ScanError> {
        let mut chars = pair.chars();
        let open = chars.next().expect("provide open bracket");
        let close = chars.next().expect("provide close bracket");
        self.skip_whitespace();
        let mut s = String::new();
        if self.ch != '\0' {
            s.push(self.ch);
        }        
        let mut level = 1;
        while let Some(c) = self.iter.next() {
            if c == open {
                level += 1;
            } else
            if c == close {
                level -= 1;
            }
            s.push(c);
            if level == 0 {
                self.nextch();
                return Ok(s);
            }
        }
        Err(self.scan_error("expect close bracket",None))

    }

}

use std::io::prelude::*;

/// used to generate Scanner structs for each line
pub struct ScanLines<R: Read> {
    rdr: io::BufReader<R>,
    line: String,
    lineno: u32,
}

impl <'a, R: Read> ScanLines<R> {

    /// create a Scanner 'iterator' over all lines from a readable.
    /// This cannot be a proper `Iterator` because the lifetime constraint
    /// on `Scanner` cannot be satisfied. You need to use the explicit form:
    ///
    /// ```rust,ignore
    /// let mut iter = ScanLines::new(File::open("lines.txt")?);
    /// while let Some(s) = iter.next() {
    ///     let mut s = s?;
    ///     // first token of each line
    ///     println!("{:?}",s.get());
    /// }
    /// ```
    pub fn new(f: R) -> ScanLines<R> {
        ScanLines {
            rdr: io::BufReader::new(f),
            line: String::new(),
            lineno: 0,
        }
    }


    /// call this to return a `Scanner` for the next line in the source.
    pub fn next(&'a mut self) -> Option<io::Result<Scanner<'a>>> {
        self.line.clear();
        match self.rdr.read_line(&mut self.line) {
            Ok(nbytes) =>  if nbytes == 0 {
                return None;
            },
            Err(e) => return Some(Err(e))
        }
        self.lineno += 1;
        Some(Ok(Scanner::new_ex(&self.line,self.lineno)))
    }

}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipping() {
        // skipping
        let mut scan = Scanner::new("here we go\nand more *yay*");
        scan.skip_until(|c| c == '\n');
        assert_eq!(scan.get(),Token::Iden("and".to_string()));
        scan.skip_until(|c| c == '*');
        assert_eq!(scan.get(),Token::Char('*'));
        assert_eq!(scan.get(),Token::Iden("yay".to_string()));
    }

    #[test]
    fn getting()  {
        use Token::*;
        let mut scan = Scanner::new("'hello' 42 * / -10 24B 2.0e6 0xFF-\"yay\"");
        assert_eq!(scan.get_string().unwrap(), "hello");
        assert_eq!(scan.get_number().unwrap(), 42.0);
        assert_eq!(scan.get_ch_matching(&['*']).unwrap(),'*');
        assert_eq!(
            scan.get_ch_matching(&[',',':']).err().unwrap(),
            ScanError::new("expected one of ',',':', got /")
        );
        assert_eq!(scan.get(),Int(-10));
        assert_eq!(scan.get(),Error(ScanError::new("bad integer: letter follows")));
        assert_eq!(scan.get(),Iden("B".to_string()));
        assert_eq!(scan.get(),Num(2000000.0));
        assert_eq!(scan.get(),Int(255));
        assert_eq!(scan.get(),Char('-'));
        assert_eq!(scan.get(),Str("yay".to_string()));
    }

    fn try_scan_err() -> Result<(),ScanError> {
        let mut scan = Scanner::new("hello: 42");
        let s = scan.get_iden()?;
        let ch = scan.get_char()?;
        let n = scan.get_integer()?;
        assert_eq!(s,"hello");
        assert_eq!(ch,':');
        assert_eq!(n,42);
        Ok(())
    }

    #[test]
    fn try_scan_test() {
        let _ = try_scan_err();
    }

    fn try_skip_chars(test: &str) -> Result<(),ScanError> {
        let mut scan = Scanner::new(test);
        scan.skip_chars("(")?;
        let name = scan.get_iden()?;
        scan.skip_chars(")=")?;
        let num = scan.get_integer()?;
        assert_eq!(name,"hello");
        assert_eq!(num,42);
        Ok(())
    }

    #[test]
    fn skip_chars() {
        let _ = try_skip_chars("(hello)=42");
        let _ = try_skip_chars(" ( hello ) =  42  ");
    }

    #[test]
    fn numbers() {
        let mut scan = Scanner::new("10 0.0 1.0e1 1e1 0 ");
        assert_eq!(scan.get_integer(),Ok(10));
        assert_eq!(scan.get_number(),Ok(0.0));
        assert_eq!(scan.get_number(),Ok(10.0));
        assert_eq!(scan.get_float(),Ok(10.0));
        assert_eq!(scan.get_integer(),Ok(0));
    }

    #[test]
    fn no_float() {
        use Token::*;
        let scan = Scanner::new("0.0 1e4").no_float();
        let c: Vec<_> = scan.collect();
        assert_eq!(c,&[Int(0),Char('.'),Int(0),Int(1),Iden("e4".into())]);
    }

    #[test]
    fn classifying_tokens() {
        let mut s = Scanner::new("10 2.0 'hello' hello?");
        let t = s.get();
        assert!(t.is_integer());
        assert!(t.is_number());
        assert!(s.get().is_float());
        assert!(s.get().is_string());
        assert!(s.get().is_iden());
        assert!(s.get().is_char());
    }

    #[test]
    fn collecting_tokens_of_type() {
        let s = Scanner::new("if let Some(a) = Bonzo::Dog {}");
        let c: Vec<_> = s.filter_map(|t| t.to_iden()).collect();
        assert_eq!(c,&["if","let","Some","a","Bonzo","Dog"]);
    }

    #[test]
    fn collecting_same_tokens_or_error() {
        let s = Scanner::new("10 1.5 20.0 30.1");
        let c: Result<Vec<_>,_> = s.map(|t| t.to_number_result()).collect();
        assert_eq!(c.unwrap(),&[10.0,1.5,20.0,30.1]);
    }

    #[test]
    fn line_comments() {
        let text = "
            one  # some comment
            20
        ";
        let mut scan = Scanner::new(text)
            .line_comment('#');
        assert_eq!(scan.get_iden(),Ok("one".into()));
        assert_eq!(scan.get_number(),Ok(20.0));
    }

}
