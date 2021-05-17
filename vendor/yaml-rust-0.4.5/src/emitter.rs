use std::convert::From;
use std::error::Error;
use std::fmt::{self, Display};
use crate::yaml::{Hash, Yaml};

#[derive(Copy, Clone, Debug)]
pub enum EmitError {
    FmtError(fmt::Error),
    BadHashmapKey,
}

impl Error for EmitError {
    fn cause(&self) -> Option<&dyn Error> {
        None
    }
}

impl Display for EmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            EmitError::FmtError(ref err) => Display::fmt(err, formatter),
            EmitError::BadHashmapKey => formatter.write_str("bad hashmap key"),
        }
    }
}

impl From<fmt::Error> for EmitError {
    fn from(f: fmt::Error) -> Self {
        EmitError::FmtError(f)
    }
}

pub struct YamlEmitter<'a> {
    writer: &'a mut dyn fmt::Write,
    best_indent: usize,
    compact: bool,

    level: isize,
}

pub type EmitResult = Result<(), EmitError>;

// from serialize::json
fn escape_str(wr: &mut dyn fmt::Write, v: &str) -> Result<(), fmt::Error> {
    wr.write_str("\"")?;

    let mut start = 0;

    for (i, byte) in v.bytes().enumerate() {
        let escaped = match byte {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            b'\x00' => "\\u0000",
            b'\x01' => "\\u0001",
            b'\x02' => "\\u0002",
            b'\x03' => "\\u0003",
            b'\x04' => "\\u0004",
            b'\x05' => "\\u0005",
            b'\x06' => "\\u0006",
            b'\x07' => "\\u0007",
            b'\x08' => "\\b",
            b'\t' => "\\t",
            b'\n' => "\\n",
            b'\x0b' => "\\u000b",
            b'\x0c' => "\\f",
            b'\r' => "\\r",
            b'\x0e' => "\\u000e",
            b'\x0f' => "\\u000f",
            b'\x10' => "\\u0010",
            b'\x11' => "\\u0011",
            b'\x12' => "\\u0012",
            b'\x13' => "\\u0013",
            b'\x14' => "\\u0014",
            b'\x15' => "\\u0015",
            b'\x16' => "\\u0016",
            b'\x17' => "\\u0017",
            b'\x18' => "\\u0018",
            b'\x19' => "\\u0019",
            b'\x1a' => "\\u001a",
            b'\x1b' => "\\u001b",
            b'\x1c' => "\\u001c",
            b'\x1d' => "\\u001d",
            b'\x1e' => "\\u001e",
            b'\x1f' => "\\u001f",
            b'\x7f' => "\\u007f",
            _ => continue,
        };

        if start < i {
            wr.write_str(&v[start..i])?;
        }

        wr.write_str(escaped)?;

        start = i + 1;
    }

    if start != v.len() {
        wr.write_str(&v[start..])?;
    }

    wr.write_str("\"")?;
    Ok(())
}

impl<'a> YamlEmitter<'a> {
    pub fn new(writer: &'a mut dyn fmt::Write) -> YamlEmitter {
        YamlEmitter {
            writer,
            best_indent: 2,
            compact: true,
            level: -1,
        }
    }

    /// Set 'compact inline notation' on or off, as described for block
    /// [sequences](http://www.yaml.org/spec/1.2/spec.html#id2797382)
    /// and
    /// [mappings](http://www.yaml.org/spec/1.2/spec.html#id2798057).
    ///
    /// In this form, blocks cannot have any properties (such as anchors
    /// or tags), which should be OK, because this emitter doesn't
    /// (currently) emit those anyways.
    pub fn compact(&mut self, compact: bool) {
        self.compact = compact;
    }

    /// Determine if this emitter is using 'compact inline notation'.
    pub fn is_compact(&self) -> bool {
        self.compact
    }

    pub fn dump(&mut self, doc: &Yaml) -> EmitResult {
        // write DocumentStart
        writeln!(self.writer, "---")?;
        self.level = -1;
        self.emit_node(doc)
    }

    fn write_indent(&mut self) -> EmitResult {
        if self.level <= 0 {
            return Ok(());
        }
        for _ in 0..self.level {
            for _ in 0..self.best_indent {
                write!(self.writer, " ")?;
            }
        }
        Ok(())
    }

    fn emit_node(&mut self, node: &Yaml) -> EmitResult {
        match *node {
            Yaml::Array(ref v) => self.emit_array(v),
            Yaml::Hash(ref h) => self.emit_hash(h),
            Yaml::String(ref v) => {
                if need_quotes(v) {
                    escape_str(self.writer, v)?;
                } else {
                    write!(self.writer, "{}", v)?;
                }
                Ok(())
            }
            Yaml::Boolean(v) => {
                if v {
                    self.writer.write_str("true")?;
                } else {
                    self.writer.write_str("false")?;
                }
                Ok(())
            }
            Yaml::Integer(v) => {
                write!(self.writer, "{}", v)?;
                Ok(())
            }
            Yaml::Real(ref v) => {
                write!(self.writer, "{}", v)?;
                Ok(())
            }
            Yaml::Null | Yaml::BadValue => {
                write!(self.writer, "~")?;
                Ok(())
            }
            // XXX(chenyh) Alias
            _ => Ok(()),
        }
    }

    fn emit_array(&mut self, v: &[Yaml]) -> EmitResult {
        if v.is_empty() {
            write!(self.writer, "[]")?;
        } else {
            self.level += 1;
            for (cnt, x) in v.iter().enumerate() {
                if cnt > 0 {
                    writeln!(self.writer)?;
                    self.write_indent()?;
                }
                write!(self.writer, "-")?;
                self.emit_val(true, x)?;
            }
            self.level -= 1;
        }
        Ok(())
    }

    fn emit_hash(&mut self, h: &Hash) -> EmitResult {
        if h.is_empty() {
            self.writer.write_str("{}")?;
        } else {
            self.level += 1;
            for (cnt, (k, v)) in h.iter().enumerate() {
                let complex_key = match *k {
                    Yaml::Hash(_) | Yaml::Array(_) => true,
                    _ => false,
                };
                if cnt > 0 {
                    writeln!(self.writer)?;
                    self.write_indent()?;
                }
                if complex_key {
                    write!(self.writer, "?")?;
                    self.emit_val(true, k)?;
                    writeln!(self.writer)?;
                    self.write_indent()?;
                    write!(self.writer, ":")?;
                    self.emit_val(true, v)?;
                } else {
                    self.emit_node(k)?;
                    write!(self.writer, ":")?;
                    self.emit_val(false, v)?;
                }
            }
            self.level -= 1;
        }
        Ok(())
    }

    /// Emit a yaml as a hash or array value: i.e., which should appear
    /// following a ":" or "-", either after a space, or on a new line.
    /// If `inline` is true, then the preceding characters are distinct
    /// and short enough to respect the compact flag.
    fn emit_val(&mut self, inline: bool, val: &Yaml) -> EmitResult {
        match *val {
            Yaml::Array(ref v) => {
                if (inline && self.compact) || v.is_empty() {
                    write!(self.writer, " ")?;
                } else {
                    writeln!(self.writer)?;
                    self.level += 1;
                    self.write_indent()?;
                    self.level -= 1;
                }
                self.emit_array(v)
            }
            Yaml::Hash(ref h) => {
                if (inline && self.compact) || h.is_empty() {
                    write!(self.writer, " ")?;
                } else {
                    writeln!(self.writer)?;
                    self.level += 1;
                    self.write_indent()?;
                    self.level -= 1;
                }
                self.emit_hash(h)
            }
            _ => {
                write!(self.writer, " ")?;
                self.emit_node(val)
            }
        }
    }
}

/// Check if the string requires quoting.
/// Strings starting with any of the following characters must be quoted.
/// :, &, *, ?, |, -, <, >, =, !, %, @
/// Strings containing any of the following characters must be quoted.
/// {, }, [, ], ,, #, `
///
/// If the string contains any of the following control characters, it must be escaped with double quotes:
/// \0, \x01, \x02, \x03, \x04, \x05, \x06, \a, \b, \t, \n, \v, \f, \r, \x0e, \x0f, \x10, \x11, \x12, \x13, \x14, \x15, \x16, \x17, \x18, \x19, \x1a, \e, \x1c, \x1d, \x1e, \x1f, \N, \_, \L, \P
///
/// Finally, there are other cases when the strings must be quoted, no matter if you're using single or double quotes:
/// * When the string is true or false (otherwise, it would be treated as a boolean value);
/// * When the string is null or ~ (otherwise, it would be considered as a null value);
/// * When the string looks like a number, such as integers (e.g. 2, 14, etc.), floats (e.g. 2.6, 14.9) and exponential numbers (e.g. 12e7, etc.) (otherwise, it would be treated as a numeric value);
/// * When the string looks like a date (e.g. 2014-12-31) (otherwise it would be automatically converted into a Unix timestamp).
fn need_quotes(string: &str) -> bool {
    fn need_quotes_spaces(string: &str) -> bool {
        string.starts_with(' ') || string.ends_with(' ')
    }

    string == ""
        || need_quotes_spaces(string)
        || string.starts_with(|character: char| match character {
            '&' | '*' | '?' | '|' | '-' | '<' | '>' | '=' | '!' | '%' | '@' => true,
            _ => false,
        })
        || string.contains(|character: char| match character {
            ':'
            | '{'
            | '}'
            | '['
            | ']'
            | ','
            | '#'
            | '`'
            | '\"'
            | '\''
            | '\\'
            | '\0'..='\x06'
            | '\t'
            | '\n'
            | '\r'
            | '\x0e'..='\x1a'
            | '\x1c'..='\x1f' => true,
            _ => false,
        })
        || [
            // http://yaml.org/type/bool.html
            // Note: 'y', 'Y', 'n', 'N', is not quoted deliberately, as in libyaml. PyYAML also parse
            // them as string, not booleans, although it is violating the YAML 1.1 specification.
            // See https://github.com/dtolnay/serde-yaml/pull/83#discussion_r152628088.
            "yes", "Yes", "YES", "no", "No", "NO", "True", "TRUE", "true", "False", "FALSE",
            "false", "on", "On", "ON", "off", "Off", "OFF",
            // http://yaml.org/type/null.html
            "null", "Null", "NULL", "~",
        ]
        .contains(&string)
        || string.starts_with('.')
        || string.starts_with("0x")
        || string.parse::<i64>().is_ok()
        || string.parse::<f64>().is_ok()
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::YamlLoader;

    #[test]
    fn test_emit_simple() {
        let s = "
# comment
a0 bb: val
a1:
    b1: 4
    b2: d
a2: 4 # i'm comment
a3: [1, 2, 3]
a4:
    - [a1, a2]
    - 2
";

        let docs = YamlLoader::load_from_str(&s).unwrap();
        let doc = &docs[0];
        let mut writer = String::new();
        {
            let mut emitter = YamlEmitter::new(&mut writer);
            emitter.dump(doc).unwrap();
        }
        println!("original:\n{}", s);
        println!("emitted:\n{}", writer);
        let docs_new = match YamlLoader::load_from_str(&writer) {
            Ok(y) => y,
            Err(e) => panic!(format!("{}", e)),
        };
        let doc_new = &docs_new[0];

        assert_eq!(doc, doc_new);
    }

    #[test]
    fn test_emit_complex() {
        let s = r#"
cataloge:
  product: &coffee   { name: Coffee,    price: 2.5  ,  unit: 1l  }
  product: &cookies  { name: Cookies!,  price: 3.40 ,  unit: 400g}

products:
  *coffee:
    amount: 4
  *cookies:
    amount: 4
  [1,2,3,4]:
    array key
  2.4:
    real key
  true:
    bool key
  {}:
    empty hash key
            "#;
        let docs = YamlLoader::load_from_str(&s).unwrap();
        let doc = &docs[0];
        let mut writer = String::new();
        {
            let mut emitter = YamlEmitter::new(&mut writer);
            emitter.dump(doc).unwrap();
        }
        let docs_new = match YamlLoader::load_from_str(&writer) {
            Ok(y) => y,
            Err(e) => panic!(format!("{}", e)),
        };
        let doc_new = &docs_new[0];
        assert_eq!(doc, doc_new);
    }

    #[test]
    fn test_emit_avoid_quotes() {
        let s = r#"---
a7: 你好
boolean: "true"
boolean2: "false"
date: 2014-12-31
empty_string: ""
empty_string1: " "
empty_string2: "    a"
empty_string3: "    a "
exp: "12e7"
field: ":"
field2: "{"
field3: "\\"
field4: "\n"
field5: "can't avoid quote"
float: "2.6"
int: "4"
nullable: "null"
nullable2: "~"
products:
  "*coffee":
    amount: 4
  "*cookies":
    amount: 4
  ".milk":
    amount: 1
  "2.4": real key
  "[1,2,3,4]": array key
  "true": bool key
  "{}": empty hash key
x: test
y: avoid quoting here
z: string with spaces"#;

        let docs = YamlLoader::load_from_str(&s).unwrap();
        let doc = &docs[0];
        let mut writer = String::new();
        {
            let mut emitter = YamlEmitter::new(&mut writer);
            emitter.dump(doc).unwrap();
        }

        assert_eq!(s, writer, "actual:\n\n{}\n", writer);
    }

    #[test]
    fn emit_quoted_bools() {
        let input = r#"---
string0: yes
string1: no
string2: "true"
string3: "false"
string4: "~"
null0: ~
[true, false]: real_bools
[True, TRUE, False, FALSE, y,Y,yes,Yes,YES,n,N,no,No,NO,on,On,ON,off,Off,OFF]: false_bools
bool0: true
bool1: false"#;
        let expected = r#"---
string0: "yes"
string1: "no"
string2: "true"
string3: "false"
string4: "~"
null0: ~
? - true
  - false
: real_bools
? - "True"
  - "TRUE"
  - "False"
  - "FALSE"
  - y
  - Y
  - "yes"
  - "Yes"
  - "YES"
  - n
  - N
  - "no"
  - "No"
  - "NO"
  - "on"
  - "On"
  - "ON"
  - "off"
  - "Off"
  - "OFF"
: false_bools
bool0: true
bool1: false"#;

        let docs = YamlLoader::load_from_str(&input).unwrap();
        let doc = &docs[0];
        let mut writer = String::new();
        {
            let mut emitter = YamlEmitter::new(&mut writer);
            emitter.dump(doc).unwrap();
        }

        assert_eq!(
            expected, writer,
            "expected:\n{}\nactual:\n{}\n",
            expected, writer
        );
    }

    #[test]
    fn test_empty_and_nested() {
        test_empty_and_nested_flag(false)
    }

    #[test]
    fn test_empty_and_nested_compact() {
        test_empty_and_nested_flag(true)
    }

    fn test_empty_and_nested_flag(compact: bool) {
        let s = if compact {
            r#"---
a:
  b:
    c: hello
  d: {}
e:
  - f
  - g
  - h: []"#
        } else {
            r#"---
a:
  b:
    c: hello
  d: {}
e:
  - f
  - g
  -
    h: []"#
        };

        let docs = YamlLoader::load_from_str(&s).unwrap();
        let doc = &docs[0];
        let mut writer = String::new();
        {
            let mut emitter = YamlEmitter::new(&mut writer);
            emitter.compact(compact);
            emitter.dump(doc).unwrap();
        }

        assert_eq!(s, writer);
    }

    #[test]
    fn test_nested_arrays() {
        let s = r#"---
a:
  - b
  - - c
    - d
    - - e
      - f"#;

        let docs = YamlLoader::load_from_str(&s).unwrap();
        let doc = &docs[0];
        let mut writer = String::new();
        {
            let mut emitter = YamlEmitter::new(&mut writer);
            emitter.dump(doc).unwrap();
        }
        println!("original:\n{}", s);
        println!("emitted:\n{}", writer);

        assert_eq!(s, writer);
    }

    #[test]
    fn test_deeply_nested_arrays() {
        let s = r#"---
a:
  - b
  - - c
    - d
    - - e
      - - f
      - - e"#;

        let docs = YamlLoader::load_from_str(&s).unwrap();
        let doc = &docs[0];
        let mut writer = String::new();
        {
            let mut emitter = YamlEmitter::new(&mut writer);
            emitter.dump(doc).unwrap();
        }
        println!("original:\n{}", s);
        println!("emitted:\n{}", writer);

        assert_eq!(s, writer);
    }

    #[test]
    fn test_nested_hashes() {
        let s = r#"---
a:
  b:
    c:
      d:
        e: f"#;

        let docs = YamlLoader::load_from_str(&s).unwrap();
        let doc = &docs[0];
        let mut writer = String::new();
        {
            let mut emitter = YamlEmitter::new(&mut writer);
            emitter.dump(doc).unwrap();
        }
        println!("original:\n{}", s);
        println!("emitted:\n{}", writer);

        assert_eq!(s, writer);
    }

}
