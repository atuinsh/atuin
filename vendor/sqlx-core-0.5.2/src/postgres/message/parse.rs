use std::i16;

use crate::io::{BufMutExt, Encode};
use crate::postgres::io::PgBufMutExt;

#[derive(Debug)]
pub struct Parse<'a> {
    /// The ID of the destination prepared statement.
    pub statement: u32,

    /// The query string to be parsed.
    pub query: &'a str,

    /// The parameter data types specified (could be zero). Note that this is not an
    /// indication of the number of parameters that might appear in the query string,
    /// only the number that the frontend wants to pre-specify types for.
    pub param_types: &'a [u32],
}

impl Encode<'_> for Parse<'_> {
    fn encode_with(&self, buf: &mut Vec<u8>, _: ()) {
        buf.push(b'P');

        buf.put_length_prefixed(|buf| {
            buf.put_statement_name(self.statement);

            buf.put_str_nul(self.query);

            // TODO: Return an error here instead
            assert!(self.param_types.len() <= (u16::MAX as usize));

            buf.extend(&(self.param_types.len() as i16).to_be_bytes());

            for &oid in self.param_types {
                buf.extend(&oid.to_be_bytes());
            }
        })
    }
}

#[test]
fn test_encode_parse() {
    const EXPECTED: &[u8] = b"P\0\0\0\x1dsqlx_s_1\0SELECT $1\0\0\x01\0\0\0\x19";

    let mut buf = Vec::new();
    let m = Parse {
        statement: 1,
        query: "SELECT $1",
        param_types: &[25],
    };

    m.encode(&mut buf);

    assert_eq!(buf, EXPECTED);
}
