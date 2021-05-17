use bytes::{Buf, Bytes};

use crate::mssql::protocol::done::Done;
use crate::mssql::protocol::login_ack::LoginAck;
use crate::mssql::protocol::order::Order;
use crate::mssql::protocol::return_status::ReturnStatus;
use crate::mssql::protocol::return_value::ReturnValue;
use crate::mssql::protocol::row::Row;

#[derive(Debug)]
pub(crate) enum Message {
    LoginAck(LoginAck),
    Done(Done),
    DoneInProc(Done),
    DoneProc(Done),
    Row(Row),
    ReturnStatus(ReturnStatus),
    ReturnValue(ReturnValue),
    Order(Order),
}

#[derive(Debug)]
pub(crate) enum MessageType {
    Info,
    LoginAck,
    EnvChange,
    Done,
    DoneProc,
    DoneInProc,
    Row,
    NbcRow,
    Error,
    ColMetaData,
    ReturnStatus,
    ReturnValue,
    Order,
}

impl MessageType {
    pub(crate) fn get(buf: &mut Bytes) -> Result<Self, crate::error::Error> {
        Ok(match buf.get_u8() {
            0x81 => MessageType::ColMetaData,
            0xaa => MessageType::Error,
            0xab => MessageType::Info,
            0xac => MessageType::ReturnValue,
            0xad => MessageType::LoginAck,
            0xd1 => MessageType::Row,
            0xd2 => MessageType::NbcRow,
            0xe3 => MessageType::EnvChange,
            0x79 => MessageType::ReturnStatus,
            0xa9 => MessageType::Order,
            0xfd => MessageType::Done,
            0xfe => MessageType::DoneProc,
            0xff => MessageType::DoneInProc,

            ty => {
                return Err(err_protocol!(
                    "unknown value `0x{:02x?}` for message type in token stream",
                    ty
                ));
            }
        })
    }
}
