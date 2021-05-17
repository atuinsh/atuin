use bitflags::bitflags;
use either::Either;

use crate::io::Encode;
use crate::mssql::io::MssqlBufMutExt;
use crate::mssql::protocol::header::{AllHeaders, Header};
use crate::mssql::MssqlArguments;

pub(crate) struct RpcRequest<'a> {
    pub(crate) transaction_descriptor: u64,

    // the procedure can be encoded as a u16 of a built-in or the name for a custom one
    pub(crate) procedure: Either<&'a str, Procedure>,
    pub(crate) options: OptionFlags,
    pub(crate) arguments: &'a MssqlArguments,
}

#[derive(Debug, Copy, Clone)]
#[repr(u16)]
#[allow(dead_code)]
pub(crate) enum Procedure {
    Cursor = 1,
    CursorOpen = 2,
    CursorPrepare = 3,
    CursorExecute = 4,
    CursorPrepareExecute = 5,
    CursorUnprepare = 6,
    CursorFetch = 7,
    CursorOption = 8,
    CursorClose = 9,
    ExecuteSql = 10,
    Prepare = 11,
    Execute = 12,
    PrepareExecute = 13,
    PrepareExecuteRpc = 14,
    Unprepare = 15,
}

bitflags! {
    pub(crate) struct OptionFlags: u16 {
        const WITH_RECOMPILE = 1;

        // The server sends NoMetaData only if fNoMetadata is set to 1 in the request
        const NO_META_DATA = 2;

        // 1 if the metadata has not changed from the previous call and the server SHOULD reuse
        // its cached metadata (the metadata MUST still be sent).
        const REUSE_META_DATA = 4;
    }
}

bitflags! {
    pub(crate) struct StatusFlags: u8 {
        // if the parameter is passed by reference (OUTPUT parameter) or
        // 0 if parameter is passed by value
        const BY_REF_VALUE = 1;

        // 1 if the parameter being passed is to be the default value
        const DEFAULT_VALUE = 2;

        // 1 if the parameter that is being passed is encrypted. This flag is valid
        // only when the column encryption feature is negotiated by client and server
        // and is turned on
        const ENCRYPTED = 8;
    }
}

impl Encode<'_> for RpcRequest<'_> {
    fn encode_with(&self, buf: &mut Vec<u8>, _: ()) {
        AllHeaders(&[Header::TransactionDescriptor {
            outstanding_request_count: 1,
            transaction_descriptor: self.transaction_descriptor,
        }])
        .encode(buf);

        match &self.procedure {
            Either::Left(name) => {
                buf.extend(&(name.len() as u16).to_le_bytes());
                buf.put_utf16_str(name);
            }

            Either::Right(id) => {
                buf.extend(&(0xffff_u16).to_le_bytes());
                buf.extend(&(*id as u16).to_le_bytes());
            }
        }

        buf.extend(&self.options.bits.to_le_bytes());
        buf.extend(&self.arguments.data);
    }
}

// TODO: Test serialization of this?
