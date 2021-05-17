use bytes::{Buf, Bytes};

use crate::error::Error;
use crate::mssql::io::MssqlBufExt;
use crate::mssql::protocol::pre_login::Version;

#[derive(Debug)]
pub(crate) struct LoginAck {
    pub(crate) interface: u8,
    pub(crate) tds_version: u32,
    pub(crate) program_name: String,
    pub(crate) program_version: Version,
}

impl LoginAck {
    pub(crate) fn get(buf: &mut Bytes) -> Result<Self, Error> {
        let len = buf.get_u16_le();
        let mut data = buf.split_to(len as usize);

        let interface = data.get_u8();
        let tds_version = data.get_u32_le();
        let program_name = data.get_b_varchar()?;
        let program_version_major = data.get_u8();
        let program_version_minor = data.get_u8();
        let program_version_build = data.get_u16();

        Ok(Self {
            interface,
            tds_version,
            program_name,
            program_version: Version {
                major: program_version_major,
                minor: program_version_minor,
                build: program_version_build,
                sub_build: 0,
            },
        })
    }
}
