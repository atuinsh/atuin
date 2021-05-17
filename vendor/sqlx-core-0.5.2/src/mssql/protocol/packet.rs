use bitflags::bitflags;
use bytes::{Buf, Bytes};

use crate::error::Error;
use crate::io::{Decode, Encode};

#[derive(Debug)]
pub(crate) struct PacketHeader {
    // Type defines the type of message. Type is a 1-byte unsigned char.
    pub(crate) r#type: PacketType,

    // Status is a bit field used to indicate the message state. Status is a 1-byte unsigned char.
    pub(crate) status: Status,

    // Length is the size of the packet including the 8 bytes in the packet header.
    pub(crate) length: u16,

    // The process ID on the server, corresponding to the current connection.
    pub(crate) server_process_id: u16,

    // Packet ID is used for numbering message packets that contain data in addition to the packet
    // header. Packet ID is a 1-byte, unsigned char. Each time packet data is sent, the value of
    // PacketID is incremented by 1, modulo 256. This allows the receiver to track the sequence
    // of TDS packets for a given message. This value is currently ignored.
    pub(crate) packet_id: u8,
}

impl<'s> Encode<'s, &'s mut usize> for PacketHeader {
    fn encode_with(&self, buf: &mut Vec<u8>, offset: &'s mut usize) {
        buf.push(self.r#type as u8);
        buf.push(self.status.bits());

        *offset = buf.len();
        buf.extend(&self.length.to_be_bytes());

        buf.extend(&self.server_process_id.to_be_bytes());
        buf.push(self.packet_id);

        // window, unused
        buf.push(0);
    }
}

impl Decode<'_> for PacketHeader {
    fn decode_with(mut buf: Bytes, _: ()) -> Result<Self, Error> {
        Ok(Self {
            r#type: PacketType::get(buf.get_u8())?,
            status: Status::from_bits_truncate(buf.get_u8()),
            length: buf.get_u16(),
            server_process_id: buf.get_u16(),
            packet_id: buf.get_u8(),
        })
    }
}

#[derive(Debug, Copy, PartialEq, Clone)]
pub(crate) enum PacketType {
    // Pre-login. Should always be #18 unless we decide to try and support pre 7.0 TDS
    PreTds7Login = 2,
    PreLogin = 18,

    SqlBatch = 1,
    Rpc = 3,
    AttentionSignal = 6,
    BulkLoadData = 7,
    FederatedAuthToken = 8,
    TransactionManagerRequest = 14,
    Tds7Login = 16,
    Sspi = 17,

    TabularResult = 4,
}

impl PacketType {
    pub fn get(value: u8) -> Result<Self, Error> {
        Ok(match value {
            1 => PacketType::SqlBatch,
            2 => PacketType::PreTds7Login,
            3 => PacketType::Rpc,
            4 => PacketType::TabularResult,
            6 => PacketType::AttentionSignal,
            7 => PacketType::BulkLoadData,
            8 => PacketType::FederatedAuthToken,
            14 => PacketType::TransactionManagerRequest,
            16 => PacketType::Tds7Login,
            17 => PacketType::Sspi,
            18 => PacketType::PreLogin,

            ty => {
                return Err(err_protocol!("unknown packet type: {}", ty));
            }
        })
    }
}

// Status is a bit field used to indicate the message state. Status is a 1-byte unsigned char.
// The following Status bit flags are defined.
bitflags! {
    pub(crate) struct Status: u8 {
        // "Normal" message.
        const NORMAL = 0x00;

        // End of message (EOM). The packet is the last packet in the whole request.
        const END_OF_MESSAGE = 0x01;

        // (From client to server) Ignore this event (0x01 MUST also be set).
        const IGNORE_EVENT = 0x02;

        // RESETCONNECTION
        //
        // (Introduced in TDS 7.1)
        //
        // (From client to server) Reset this connection
        // before processing event. Only set for event types Batch, RPC, or Transaction Manager
        // request. If clients want to set this bit, it MUST be part of the first packet of the
        // message. This signals the server to clean up the environment state of the connection
        // back to the default environment setting, effectively simulating a logout and a
        // subsequent login, and provides server support for connection pooling. This bit SHOULD
        // be ignored if it is set in a packet that is not the first packet of the message.
        //
        // This status bit MUST NOT be set in conjunction with the RESETCONNECTIONSKIPTRAN bit.
        // Distributed transactions and isolation levels will not be reset.
        const RESET_CONN = 0x08;

        // RESETCONNECTIONSKIPTRAN
        //
        // (Introduced in TDS 7.3)
        //
        // (From client to server) Reset the
        // connection before processing event but do not modify the transaction state (the
        // state will remain the same before and after the reset). The transaction in the
        // session can be a local transaction that is started from the session or it can
        // be a distributed transaction in which the session is enlisted. This status bit
        // MUST NOT be set in conjunction with the RESETCONNECTION bit.
        // Otherwise identical to RESETCONNECTION.
        const RESET_CONN_SKIP_TRAN = 0x10;
    }
}
