use crate::io::Encode;
use crate::mysql::protocol::Capabilities;

// https://dev.mysql.com/doc/internals/en/com-stmt-close.html

#[derive(Debug)]
pub struct StmtClose {
    pub statement: u32,
}

impl Encode<'_, Capabilities> for StmtClose {
    fn encode_with(&self, buf: &mut Vec<u8>, _: Capabilities) {
        buf.push(0x19); // COM_STMT_CLOSE
        buf.extend(&self.statement.to_le_bytes());
    }
}
