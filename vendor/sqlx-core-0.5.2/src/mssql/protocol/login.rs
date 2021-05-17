use crate::io::Encode;
use crate::mssql::io::MssqlBufMutExt;

#[derive(Debug)]
pub struct Login7<'a> {
    pub version: u32,
    pub packet_size: u32,
    pub client_program_version: u32,
    pub client_pid: u32,
    pub hostname: &'a str,
    pub username: &'a str,
    pub password: &'a str,
    pub app_name: &'a str,
    pub server_name: &'a str,
    pub client_interface_name: &'a str,
    pub language: &'a str,
    pub database: &'a str,
    pub client_id: [u8; 6],
}

impl Encode<'_> for Login7<'_> {
    fn encode_with(&self, buf: &mut Vec<u8>, _: ()) {
        // [Length] The total length of the LOGIN7 structure.
        let beg = buf.len();
        buf.extend(&0_u32.to_le_bytes());

        // [TDSVersion] The highest TDS version supported by the client.
        buf.extend(&self.version.to_le_bytes());

        // [PacketSize] The packet size being requested by the client.
        buf.extend(&self.packet_size.to_le_bytes());

        // [ClientProgVer] The version of the **interface** library.
        buf.extend(&self.client_program_version.to_le_bytes());

        // [ClientPID] The process ID of the client application.
        buf.extend(&self.client_pid.to_le_bytes());

        // [ConnectionID] The connection ID of the primary server.
        buf.extend(&0_u32.to_le_bytes());

        // [OptionFlags1]
        //    7 | SET_LANG_ON (1) – Require a warning message for a language choice statement
        //    6 | INIT_DB_FATAL (1) – Fail to change to initial database should be fatal
        //    5 | USE_DB_ON (1) – Require a warning message for a db change statement
        //    4 | DUMPLOAD_OFF (0)
        //  3-2 | FLOAT_IEEE_754 (0)
        //    1 | CHARSET_ASCII (0)
        //    0 | ORDER_X86 (0)
        buf.push(0b11_10_00_00);

        // [OptionsFlags2]
        //    6 | INTEGRATED_SECURITY_OFF (0)
        //  5-4 | USER_NORMAL (0)
        //    3 | <fCacheConnect>
        //    2 | <fTransBoundary>
        //    1 | ODBC_ON (1)
        //    0 | INIT_LANG_FATAL (1)
        buf.push(0b00_00_00_11);

        // [TypeFlags]
        //    2 | <fReadOnlyIntent>
        //    1 | OLEDB_OFF (0)
        //    0 | SQL_DFLT (0)
        buf.push(0);

        // [OptionFlags3]
        //    4 | <fExtension>
        //    3 | <fUnknownCollationHandling>
        //    2 | <fUserInstance>
        //    1 | <fSendYukonBinaryXML>
        //    0 | <fChangePassword>
        buf.push(0);

        // [ClientTimeZone] This field is not used and can be set to zero.
        buf.extend(&0_u32.to_le_bytes());

        // [ClientLanguageCodeIdentifier] The language code identifier (LCID) value for
        //   the client collation.
        buf.extend(&0_u32.to_le_bytes());

        // [OffsetLength] pre-allocate a space for all offset, length pairs
        let mut offsets = buf.len();
        buf.resize(buf.len() + 58, 0);

        // [Hostname] The client machine name
        write_str(buf, &mut offsets, beg, self.hostname);

        // [UserName] The client user ID
        write_str(buf, &mut offsets, beg, self.username);

        // [Password] The password supplied by the client
        let password_start = buf.len();
        write_str(buf, &mut offsets, beg, self.password);

        // Before submitting a password from the client to the server, for every byte in the
        // password buffer starting with the position pointed to by ibPassword or
        // ibChangePassword, the client SHOULD first swap the four high bits with
        // the four low bits and then do a bit-XOR with 0xA5 (10100101).
        for i in password_start..buf.len() {
            let b = buf[i];
            buf[i] = ((b << 4) & 0xf0 | (b >> 4) & 0x0f) ^ 0xa5;
        }

        // [AppName] The client application name
        write_str(buf, &mut offsets, beg, self.app_name);

        // [ServerName] The server name
        write_str(buf, &mut offsets, beg, self.server_name);

        // [Extension] Points to an extension block.
        // TODO: Implement to get FeatureExt which should let us use UTF-8
        write_offset(buf, &mut offsets, beg);
        offsets += 2;

        // [CltIntName] The interface library name
        write_str(buf, &mut offsets, beg, self.client_interface_name);

        // [Language] The initial language (overrides the user IDs language)
        write_str(buf, &mut offsets, beg, self.language);

        // [Database] The initial database (overrides the user IDs database)
        write_str(buf, &mut offsets, beg, self.database);

        // [ClientID] The unique client ID. Can be all zero.
        buf[offsets..(offsets + 6)].copy_from_slice(&self.client_id);
        offsets += 6;

        // [SSPI] SSPI data
        write_offset(buf, &mut offsets, beg);
        offsets += 2;

        // [AtchDBFile] The file name for a database that is to be attached
        write_offset(buf, &mut offsets, beg);
        offsets += 2;

        // [ChangePassword] New password for the specified login
        write_offset(buf, &mut offsets, beg);

        // Establish the length of the entire structure
        let len = buf.len();
        buf[beg..beg + 4].copy_from_slice(&((len - beg) as u32).to_le_bytes());
    }
}

fn write_offset(buf: &mut Vec<u8>, offsets: &mut usize, beg: usize) {
    // The offset must be relative to the beginning of the packet payload, after
    // the packet header

    let offset = buf.len() - beg;
    buf[*offsets..(*offsets + 2)].copy_from_slice(&(offset as u16).to_le_bytes());

    *offsets += 2;
}

fn write_str(buf: &mut Vec<u8>, offsets: &mut usize, beg: usize, s: &str) {
    // Write the offset
    write_offset(buf, offsets, beg);

    // Write the length, in UCS-2 characters
    buf[*offsets..(*offsets + 2)].copy_from_slice(&(s.len() as u16).to_le_bytes());
    *offsets += 2;

    // Encode the character sequence as UCS-2 (precursor to UTF16-LE)
    buf.put_utf16_str(s);
}

#[test]
fn test_encode_login() {
    let mut buf = Vec::new();

    let login = Login7 {
        version: 0x72090002,
        client_program_version: 0x07_00_00_00,
        client_pid: 0x0100,
        packet_size: 0x1000,
        hostname: "skostov1",
        username: "sa",
        password: "",
        app_name: "OSQL-32",
        server_name: "",
        client_interface_name: "ODBC",
        language: "",
        database: "",
        client_id: [0x00, 0x50, 0x8B, 0xE2, 0xB7, 0x8F],
    };

    // Adapted from v20191101 of MS-TDS
    #[rustfmt::skip]
    let expected = vec![
        // Packet Header
        /* 0x10, 0x01, 0x00, 0x90, 0x00, 0x00, 0x01, 0x00, */

        0x88, 0x00, 0x00, 0x00, // Length
        0x02, 0x00, 0x09, 0x72, // TDS Version = SQL Server 2005
        0x00, 0x10, 0x00, 0x00, // Packet Size = 1048576 or 1 Mi
        0x00, 0x00, 0x00, 0x07, // Client Program Version = 7
        0x00, 0x01, 0x00, 0x00, // Client PID = 0x01_00_00
        0x00, 0x00, 0x00, 0x00, // Connection ID
        0xE0,                   // [OptionFlags1] 0b1110_0000
        0x03,                   // [OptionFlags2] 0b0000_0011
        0x00,                   // [TypeFlags]
        0x00,                   // [OptionFlags3]
        0x00, 0x00, 0x00, 0x00, // [ClientTimeZone]
        0x00, 0x00, 0x00, 0x00, // [ClientLCID]
        0x5E, 0x00,             // [ibHostName]
        0x08, 0x00,             // [cchHostName]
        0x6E, 0x00,             // [ibUserName]
        0x02, 0x00,             // [cchUserName]
        0x72, 0x00,             // [ibPassword]
        0x00, 0x00,             // [cchPassword]
        0x72, 0x00,             // [ibAppName]
        0x07, 0x00,             // [cchAppName]
        0x80, 0x00,             // [ibServerName]
        0x00, 0x00,             // [cchServerName]
        0x80, 0x00,             // [ibUnused]
        0x00, 0x00,             // [cbUnused]
        0x80, 0x00,             // [ibCltIntName]
        0x04, 0x00,             // [cchCltIntName]
        0x88, 0x00,             // [ibLanguage]
        0x00, 0x00,             // [cchLanguage]
        0x88, 0x00,             // [ibDatabase]
        0x00, 0x00,             // [chDatabase]
        0x00, 0x50, 0x8B,       // [ClientID]
        0xE2, 0xB7, 0x8F,
        0x88, 0x00,             // [ibSSPI]
        0x00, 0x00,             // [cchSSPI]
        0x88, 0x00,             // [ibAtchDBFile]
        0x00, 0x00,             // [cchAtchDBFile]
        0x88, 0x00,             // [ibChangePassword]
        0x00, 0x00,             // [cchChangePassword]
        0x00, 0x00, 0x00, 0x00, // [cbSSPILong]
        0x73, 0x00, 0x6B, 0x00, 0x6F, 0x00, 0x73, 0x00, 0x74, 0x00, // [Data]
        0x6F, 0x00, 0x76, 0x00, 0x31, 0x00, 0x73, 0x00, 0x61, 0x00,
        0x4F, 0x00, 0x53, 0x00, 0x51, 0x00, 0x4C, 0x00, 0x2D, 0x00,
        0x33, 0x00, 0x32, 0x00, 0x4F, 0x00, 0x44, 0x00, 0x42, 0x00,
        0x43, 0x00,
    ];

    login.encode(&mut buf);

    assert_eq!(expected, buf);
}
