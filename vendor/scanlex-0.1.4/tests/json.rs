// A Simple, toy JSON parser.
// Remember there are Crates for This
extern crate scanlex;
use scanlex::{Scanner,Token,ScanError};

use std::collections::HashMap;

type JsonArray = Vec<Box<Value>>;
type JsonObject = HashMap<String,Box<Value>>;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
   Str(String),
   Num(f64),
   Bool(bool),
   Arr(JsonArray),
   Obj(JsonObject),
   Null
}

fn scan_json(scan: &mut Scanner) -> Result<Value,ScanError> {
    use Value::*;
    match scan.get() {
        Token::Str(s) => Ok(Str(s)),
        Token::Num(x) => Ok(Num(x)),
        Token::Int(n) => Ok(Num(n as f64)),
        Token::End => Err(scan.scan_error("unexpected end of input",None)),
        Token::Error(e) => Err(e),
        Token::Iden(s) =>
            if s == "null"    {Ok(Null)}
            else if s == "true" {Ok(Bool(true))}
            else if s == "false" {Ok(Bool(false))}
            else {Err(scan.scan_error(&format!("unknown identifier '{}'",s),None))},
        Token::Char(c) =>
            if c == '[' {
                let mut ja = Vec::new();
                let mut ch = c;
                while ch != ']' {
                    let o = scan_json(scan)?;
                    ch = scan.get_ch_matching(&[',',']'])?;
                    ja.push(Box::new(o));
                }
                Ok(Arr(ja))
            } else
            if c == '{' {
                let mut jo = HashMap::new();
                let mut ch = c;
                while ch != '}' {
                    let key = scan.get_string()?;
                    scan.get_ch_matching(&[':'])?;
                    let o = scan_json(scan)?;
                    ch = scan.get_ch_matching(&[',','}'])?;
                    jo.insert(key,Box::new(o));
                }
                Ok(Obj(jo))
            } else {
                Err(scan.scan_error(&format!("bad char '{}'",c),None))
            }
    }
}

fn parse_json(txt: &str) -> Value {
    let mut scan = Scanner::new(txt);
    scan_json(&mut scan).expect("bad json")
}

use Value::*;

#[test]
fn array() {
	let s = parse_json("[10,20]");
    assert_eq!(s, Arr(vec![Box::new(Num(10.0)),Box::new(Num(20.0))]));
}


#[test]
fn array2() {
	let s = parse_json("[null,true]");
    assert_eq!(s, Arr(vec![Box::new(Null),Box::new(Bool(true))]));
}

#[test]
fn map() {
	let s = parse_json("{'bonzo':10}");
    let mut m = HashMap::new();
    m.insert("bonzo".to_string(),Box::new(Num(10.0)));
	assert_eq!(s, Obj(m));
}




