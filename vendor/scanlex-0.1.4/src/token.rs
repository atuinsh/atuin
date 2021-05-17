use error::ScanError;
use int::Int;

/// Represents a token returned by `Scanner::get`
#[derive(Debug)]
#[derive(PartialEq)]
pub enum Token {
  /// a floating-point number, stored as double-precision float
  Num(f64),
  /// an integer, stored as eight-byte unsigned
  Int(i64),
  /// a quoted string
  Str(String),
  /// an identifier \a+[\a\d_]*
  Iden(String),
  /// a character (anything not recognized as any of the above
  Char(char),
  /// represents an error
  Error(ScanError),
  /// end of stream
  End
}

fn type_error<T>(t: Token, expected: &str) -> Result<T,ScanError> {
    Err(ScanError{details: format!("{} expected, got {:?}",expected,t), lineno: 1})
}

fn int_error<T>(msg: &str, tname: &str) -> Result<T,ScanError> {
    Err(ScanError{details: format!("integer {} for {}",msg,tname), lineno: 1})
}

impl Token {
    /// is this the end token?
    pub fn finished(&self) -> bool {
        match *self {
            Token::End => true,
            _ => false
        }
    }

    /// is this token a float?
    pub fn is_float(&self) -> bool {
        match *self {
            Token::Num(_) => true,
            _ => false
        }
    }

    /// extract the float
    pub fn to_float(self) -> Option<f64> {
        match self {
            Token::Num(n) => Some(n),
            _ => None
        }
    }

    /// extract the float, or complain
    pub fn to_float_result(self) -> Result<f64,ScanError> {
        match self {
            Token::Num(n) => Ok(n),
            t => type_error(t,"float")
        }
    }

    /// is this token an integer?
    pub fn is_integer(&self) -> bool {
        match *self {
            Token::Int(_) => true,
            _ => false
        }
    }

    /// extract the integer
    pub fn to_integer(self) -> Option<i64> {
        match self {
            Token::Int(n) => Some(n),
            _ => None
        }
    }

    /// extract the integer, or complain
    pub fn to_integer_result(self) -> Result<i64,ScanError> {
        match self {
            Token::Int(n) => Ok(n),
            t => type_error(t,"integer")
        }
    }

    /// extract the integer as a particular subtype
    pub fn to_int_result<I: Int>(self) -> Result<I::Type,ScanError> {
        let num = self.to_integer_result()?;
        if num < I::min_value() {
            return int_error("underflow",I::name());
        } else
        if num > I::max_value() {
            return int_error("overflow",I::name());
        }
        Ok(I::cast(num))
    }

    /// is this token an integer?
    pub fn is_number(&self) -> bool {
        match *self {
            Token::Int(_) | Token::Num(_) => true,
            _ => false
        }
    }

    /// extract the number, not caring about float or integer
    pub fn to_number(self) -> Option<f64> {
        match self {
            Token::Num(n) => Some(n),
            Token::Int(n) => Some(n as f64),
            _ => None
        }
    }

    /// extract the number, not caring about float or integer, or complain
    pub fn to_number_result(self) -> Result<f64,ScanError> {
        match self {
            Token::Num(n) => Ok(n),
            Token::Int(n) => Ok(n as f64),
            t => type_error(t,"number")
        }
    }

    /// is this token a string?
    pub fn is_string(&self) -> bool {
        match *self {
            Token::Str(_) => true,
            _ => false
        }
    }

    /// extract the string
    pub fn to_string(self) -> Option<String> {
        match self {
            Token::Str(s) => Some(s),
            _ => None
        }
    }

    /// extract a reference the string
    pub fn as_string(&self) -> Option<&str> {
        match *self {
            Token::Str(ref s) => Some(s.as_str()),
            _ => None
        }
    }

    /// extract the string, or complain
    pub fn to_string_result(self) -> Result<String,ScanError> {
        match self {
            Token::Str(s) => Ok(s),
            t => type_error(t,"string")
        }
    }

    /// is this token an identifier?
    pub fn is_iden(&self) -> bool {
        match *self {
            Token::Iden(_) => true,
            _ => false
        }
    }

    /// extract the identifier
    pub fn to_iden(self) -> Option<String> {
        match self {
            Token::Iden(n) => Some(n),
            _ => None
        }
    }

    /// extract a reference to the identifier
    pub fn as_iden(&self) -> Option<&str> {
        match *self {
            Token::Iden(ref n) => Some(n.as_str()),
            _ => None
        }
    }


    /// extract the identifier, or complain
    pub fn to_iden_result(self) -> Result<String,ScanError> {
        match self {
            Token::Iden(n) => Ok(n),
            t => type_error(t,"iden")
        }
    }

    /// is this token a character?
    pub fn is_char(&self) -> bool {
        match *self {
            Token::Char(_) => true,
            _ => false
        }
    }

    /// extract the character
    pub fn to_char(self) -> Option<char> {
        match self {
            Token::Char(c) => Some(c),
            _ => None
        }
    }

    /// extract the character
    pub fn as_char(&self) -> Option<char> {
        match *self {
            Token::Char(c) => Some(c),
            _ => None
        }
    }

    /// extract the character, or complain
    pub fn to_char_result(self) -> Result<char,ScanError> {
        match self {
            Token::Char(c) => Ok(c),
            t => type_error(t,"char")
        }
    }

    /// is this token an error?
    pub fn is_error(&self) -> bool {
        match *self {
            Token::Error(_) => true,
            _ => false
        }
    }

    /// extract the error
    pub fn to_error(self) -> Option<ScanError> {
        match self {
            Token::Error(e) => Some(e),
            _ => None
        }
    }

}
