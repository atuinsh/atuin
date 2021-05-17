use std::fmt::{self, Display, Formatter};

use bitflags::bitflags;
use bytes::{Buf, Bytes};
use uuid::Uuid;

use crate::error::Error;
use crate::io::{Decode, Encode};

/// A message sent by the client to set up context for login. The server responds to a client
/// `PRELOGIN` message with a message of packet header type `0x04` and the packet data
/// containing a `PRELOGIN` structure.
#[derive(Debug, Default)]
pub(crate) struct PreLogin<'a> {
    pub(crate) version: Version,
    pub(crate) encryption: Encrypt,
    pub(crate) instance: Option<&'a str>,
    pub(crate) thread_id: Option<u32>,
    pub(crate) trace_id: Option<TraceId>,
    pub(crate) multiple_active_result_sets: Option<bool>,
}

impl<'de> Decode<'de> for PreLogin<'de> {
    fn decode_with(buf: Bytes, _: ()) -> Result<Self, Error> {
        let mut version = None;
        let mut encryption = None;

        // TODO: Decode the remainder of the structure
        // let mut instance = None;
        // let mut thread_id = None;
        // let mut trace_id = None;
        // let mut multiple_active_result_sets = None;

        let mut offsets = buf.clone();

        loop {
            let token = offsets.get_u8();

            match PreLoginOptionToken::get(token) {
                Some(token) => {
                    let offset = offsets.get_u16() as usize;
                    let size = offsets.get_u16() as usize;
                    let mut data = &buf[offset..offset + size];

                    match token {
                        PreLoginOptionToken::Version => {
                            let major = data.get_u8();
                            let minor = data.get_u8();
                            let build = data.get_u16();
                            let sub_build = data.get_u16();

                            version = Some(Version {
                                major,
                                minor,
                                build,
                                sub_build,
                            });
                        }

                        PreLoginOptionToken::Encryption => {
                            encryption = Some(Encrypt::from_bits_truncate(data.get_u8()));
                        }

                        tok => todo!("{:?}", tok),
                    }
                }

                None if token == 0xff => {
                    break;
                }

                None => {
                    return Err(err_protocol!(
                        "PRELOGIN: unexpected login option token: 0x{:02?}",
                        token
                    )
                    .into());
                }
            }
        }

        let version =
            version.ok_or(err_protocol!("PRELOGIN: missing required `version` option"))?;

        let encryption = encryption.ok_or(err_protocol!(
            "PRELOGIN: missing required `encryption` option"
        ))?;

        Ok(Self {
            version,
            encryption,

            ..Default::default()
        })
    }
}

impl Encode<'_> for PreLogin<'_> {
    fn encode_with(&self, buf: &mut Vec<u8>, _: ()) {
        use PreLoginOptionToken::*;

        // NOTE: Packet headers are written in MssqlStream::write

        // Rules
        //  PRELOGIN = (*PRELOGIN_OPTION *PL_OPTION_DATA) / SSL_PAYLOAD
        //  PRELOGIN_OPTION = (PL_OPTION_TOKEN PL_OFFSET PL_OPTION_LENGTH) / TERMINATOR

        // Count the number of set options
        let num_options = 2
            + self.instance.map_or(0, |_| 1)
            + self.thread_id.map_or(0, |_| 1)
            + self.trace_id.as_ref().map_or(0, |_| 1)
            + self.multiple_active_result_sets.map_or(0, |_| 1);

        // Calculate the length of the option offset block. Each block is 5 bytes and it ends in
        // a 1 byte terminator.
        let len_offsets = (num_options * 5) + 1;
        let mut offsets = buf.len() as usize;
        let mut offset = len_offsets as u16;

        // Reserve a chunk for the offset block and set the final terminator
        buf.resize(buf.len() + len_offsets, 0);
        let end_offsets = buf.len() - 1;
        buf[end_offsets] = 0xff;

        // NOTE: VERSION is a required token, and it MUST be the first token.
        Version.put(buf, &mut offsets, &mut offset, 6);
        self.version.encode(buf);

        Encryption.put(buf, &mut offsets, &mut offset, 1);
        buf.push(self.encryption.bits());

        if let Some(name) = self.instance {
            Instance.put(buf, &mut offsets, &mut offset, name.len() as u16 + 1);
            buf.extend_from_slice(name.as_bytes());
            buf.push(b'\0');
        }

        if let Some(id) = self.thread_id {
            ThreadId.put(buf, &mut offsets, &mut offset, 4);
            buf.extend_from_slice(&id.to_le_bytes());
        }

        if let Some(trace) = &self.trace_id {
            ThreadId.put(buf, &mut offsets, &mut offset, 36);
            buf.extend_from_slice(trace.connection_id.as_bytes());
            buf.extend_from_slice(trace.activity_id.as_bytes());
            buf.extend_from_slice(&trace.activity_seq.to_be_bytes());
        }

        if let Some(mars) = &self.multiple_active_result_sets {
            MultipleActiveResultSets.put(buf, &mut offsets, &mut offset, 1);
            buf.push(*mars as u8);
        }
    }
}

// token value representing the option (PL_OPTION_TOKEN)
#[derive(Debug, Copy, Clone)]
#[repr(u8)]
enum PreLoginOptionToken {
    Version = 0x00,
    Encryption = 0x01,
    Instance = 0x02,
    ThreadId = 0x03,

    // Multiple Active Result Sets (MARS)
    MultipleActiveResultSets = 0x04,

    TraceId = 0x05,
}

impl PreLoginOptionToken {
    fn put(self, buf: &mut Vec<u8>, pos: &mut usize, offset: &mut u16, len: u16) {
        buf[*pos] = self as u8;
        *pos += 1;

        buf[*pos..(*pos + 2)].copy_from_slice(&offset.to_be_bytes());
        *pos += 2;

        buf[*pos..(*pos + 2)].copy_from_slice(&len.to_be_bytes());
        *pos += 2;

        *offset += len;
    }

    fn get(b: u8) -> Option<Self> {
        Some(match b {
            0x00 => PreLoginOptionToken::Version,
            0x01 => PreLoginOptionToken::Encryption,
            0x02 => PreLoginOptionToken::Instance,
            0x03 => PreLoginOptionToken::ThreadId,
            0x04 => PreLoginOptionToken::MultipleActiveResultSets,
            0x05 => PreLoginOptionToken::TraceId,

            _ => {
                return None;
            }
        })
    }
}

#[derive(Debug)]
pub(crate) struct TraceId {
    // client application trace ID (GUID_CONNID)
    pub(crate) connection_id: Uuid,

    // client application activity ID (GUID_ActivityID)
    pub(crate) activity_id: Uuid,

    // client application activity sequence (ActivitySequence)
    pub(crate) activity_seq: u32,
}

// Version of the sender (UL_VERSION)
#[derive(Debug, Default)]
pub(crate) struct Version {
    pub(crate) major: u8,
    pub(crate) minor: u8,
    pub(crate) build: u16,

    // Sub-build number of the sender (US_SUBBUILD)
    pub(crate) sub_build: u16,
}

impl Version {
    fn encode(&self, buf: &mut Vec<u8>) {
        buf.push(self.major);
        buf.push(self.minor);
        buf.extend(&self.build.to_be_bytes());
        buf.extend(&self.sub_build.to_be_bytes());
    }
}

impl Display for Version {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}.{}", self.major, self.minor, self.build)
    }
}

bitflags! {
    /// During the Pre-Login handshake, the client and the server negotiate the
    /// wire encryption to be used.
    #[derive(Default)]
    pub(crate) struct Encrypt: u8 {
        /// Encryption is available but on.
        const ON = 0x01;

        /// Encryption is not available.
        const NOT_SUPPORTED = 0x02;

        /// Encryption is required.
        const REQUIRED = 0x03;

        /// The client certificate should be used to authenticate
        /// the user in place of a user/password.
        const CLIENT_CERT = 0x80;
    }
}

#[test]
fn test_encode_pre_login() {
    let mut buf = Vec::new();

    let pre_login = PreLogin {
        version: Version {
            major: 9,
            minor: 0,
            build: 0,
            sub_build: 0,
        },
        encryption: Encrypt::ON,
        instance: Some(""),
        thread_id: Some(0x00000DB8),
        multiple_active_result_sets: Some(true),

        ..Default::default()
    };

    // From v20191101 of MS-TDS documentation
    #[rustfmt::skip]
    let expected = vec![
        0x00, 0x00, 0x1A, 0x00, 0x06, 0x01, 0x00, 0x20, 0x00, 0x01, 0x02, 0x00, 0x21, 0x00,
        0x01, 0x03, 0x00, 0x22, 0x00, 0x04, 0x04, 0x00, 0x26, 0x00, 0x01, 0xFF, 0x09, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0xB8, 0x0D, 0x00, 0x00, 0x01
    ];

    pre_login.encode(&mut buf);

    assert_eq!(expected, buf);
}

#[test]
fn test_decode_pre_login() {
    #[rustfmt::skip]
    let buffer = Bytes::from_static(&[
        0, 0, 11, 0, 6, 1, 0, 17, 0, 1, 255,
        14, 0, 12, 209, 0, 0, 0,
    ]);

    let pre_login = PreLogin::decode(buffer).unwrap();

    // v14.0.3281
    assert_eq!(pre_login.version.major, 14);
    assert_eq!(pre_login.version.minor, 0);
    assert_eq!(pre_login.version.build, 3281);
    assert_eq!(pre_login.version.sub_build, 0);

    // ENCRYPT_OFF
    assert_eq!(pre_login.encryption.bits(), 0);
}
