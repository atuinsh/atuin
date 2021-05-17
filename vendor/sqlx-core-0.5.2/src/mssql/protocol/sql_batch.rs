use crate::io::Encode;
use crate::mssql::io::MssqlBufMutExt;
use crate::mssql::protocol::header::{AllHeaders, Header};

#[derive(Debug)]
pub(crate) struct SqlBatch<'a> {
    pub(crate) transaction_descriptor: u64,
    pub(crate) sql: &'a str,
}

impl Encode<'_> for SqlBatch<'_> {
    fn encode_with(&self, buf: &mut Vec<u8>, _: ()) {
        AllHeaders(&[Header::TransactionDescriptor {
            outstanding_request_count: 1,
            transaction_descriptor: self.transaction_descriptor,
        }])
        .encode(buf);

        // SQLText
        buf.put_utf16_str(self.sql);
    }
}
