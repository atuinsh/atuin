use bytes::{Buf, Bytes};

use crate::error::Error;
use crate::mssql::io::MssqlBufExt;

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum EnvChange {
    Database(String),
    Language(String),
    CharacterSet(String),
    PacketSize(String),
    UnicodeDataSortingLocalId(String),
    UnicodeDataSortingComparisonFlags(String),
    SqlCollation(Bytes),

    // TDS 7.2+
    BeginTransaction(u64),
    CommitTransaction(u64),
    RollbackTransaction(u64),
    EnlistDtcTransaction,
    DefectTransaction,
    RealTimeLogShipping,
    PromoteTransaction,
    TransactionManagerAddress,
    TransactionEnded,
    ResetConnectionCompletionAck,
    LoginRequestUserNameAck,

    // TDS 7.4+
    RoutingInformation,
}

impl EnvChange {
    pub(crate) fn get(buf: &mut Bytes) -> Result<Self, Error> {
        let len = buf.get_u16_le();
        let ty = buf.get_u8();
        let mut data = buf.split_to((len - 1) as usize);

        Ok(match ty {
            1 => EnvChange::Database(data.get_b_varchar()?),
            2 => EnvChange::Language(data.get_b_varchar()?),
            3 => EnvChange::CharacterSet(data.get_b_varchar()?),
            4 => EnvChange::PacketSize(data.get_b_varchar()?),
            5 => EnvChange::UnicodeDataSortingLocalId(data.get_b_varchar()?),
            6 => EnvChange::UnicodeDataSortingComparisonFlags(data.get_b_varchar()?),
            7 => EnvChange::SqlCollation(data.get_b_varbyte()),
            8 => EnvChange::BeginTransaction(data.get_b_varbyte().get_u64_le()),

            9 => {
                let _ = data.get_u8();
                EnvChange::CommitTransaction(data.get_u64_le())
            }

            10 => {
                let _ = data.get_u8();
                EnvChange::RollbackTransaction(data.get_u64_le())
            }

            _ => {
                return Err(err_protocol!("unexpected value {} for ENVCHANGE Type", ty));
            }
        })
    }
}
