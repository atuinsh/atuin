use atuin_common::record::{EncryptedData, Record};
use atuin_server_database::models::{History, Session, User};

pub struct DbUser(pub User);
pub struct DbSession(pub Session);
pub struct DbHistory(pub History);
pub struct DbRecord(pub Record<EncryptedData>);
