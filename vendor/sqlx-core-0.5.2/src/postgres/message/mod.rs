use bytes::Bytes;

use crate::error::Error;
use crate::io::Decode;

mod authentication;
mod backend_key_data;
mod bind;
mod close;
mod command_complete;
mod data_row;
mod describe;
mod execute;
mod flush;
mod notification;
mod parameter_description;
mod parse;
mod password;
mod query;
mod ready_for_query;
mod response;
mod row_description;
mod sasl;
mod ssl_request;
mod startup;
mod sync;
mod terminate;

pub use authentication::{Authentication, AuthenticationSasl};
pub use backend_key_data::BackendKeyData;
pub use bind::Bind;
pub use close::Close;
pub use command_complete::CommandComplete;
pub use data_row::DataRow;
pub use describe::Describe;
pub use execute::Execute;
pub use flush::Flush;
pub use notification::Notification;
pub use parameter_description::ParameterDescription;
pub use parse::Parse;
pub use password::Password;
pub use query::Query;
pub use ready_for_query::{ReadyForQuery, TransactionStatus};
pub use response::{Notice, PgSeverity};
pub use row_description::RowDescription;
pub use sasl::{SaslInitialResponse, SaslResponse};
pub use ssl_request::SslRequest;
pub use startup::Startup;
pub use sync::Sync;
pub use terminate::Terminate;

#[derive(Debug, PartialOrd, PartialEq)]
#[repr(u8)]
pub enum MessageFormat {
    Authentication,
    BackendKeyData,
    BindComplete,
    CloseComplete,
    CommandComplete,
    DataRow,
    EmptyQueryResponse,
    ErrorResponse,
    NoData,
    NoticeResponse,
    NotificationResponse,
    ParameterDescription,
    ParameterStatus,
    ParseComplete,
    PortalSuspended,
    ReadyForQuery,
    RowDescription,
}

#[derive(Debug)]
pub struct Message {
    pub format: MessageFormat,
    pub contents: Bytes,
}

impl Message {
    #[inline]
    pub fn decode<'de, T>(self) -> Result<T, Error>
    where
        T: Decode<'de>,
    {
        T::decode(self.contents)
    }
}

impl MessageFormat {
    pub fn try_from_u8(v: u8) -> Result<Self, Error> {
        // https://www.postgresql.org/docs/current/protocol-message-formats.html

        Ok(match v {
            b'1' => MessageFormat::ParseComplete,
            b'2' => MessageFormat::BindComplete,
            b'3' => MessageFormat::CloseComplete,
            b'C' => MessageFormat::CommandComplete,
            b'D' => MessageFormat::DataRow,
            b'E' => MessageFormat::ErrorResponse,
            b'I' => MessageFormat::EmptyQueryResponse,
            b'A' => MessageFormat::NotificationResponse,
            b'K' => MessageFormat::BackendKeyData,
            b'N' => MessageFormat::NoticeResponse,
            b'R' => MessageFormat::Authentication,
            b'S' => MessageFormat::ParameterStatus,
            b'T' => MessageFormat::RowDescription,
            b'Z' => MessageFormat::ReadyForQuery,
            b'n' => MessageFormat::NoData,
            b's' => MessageFormat::PortalSuspended,
            b't' => MessageFormat::ParameterDescription,

            _ => return Err(err_protocol!("unknown message type: {:?}", v as char)),
        })
    }
}
