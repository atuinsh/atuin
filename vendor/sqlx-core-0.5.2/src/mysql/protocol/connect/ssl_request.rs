use crate::io::Encode;
use crate::mysql::protocol::Capabilities;

// https://dev.mysql.com/doc/dev/mysql-server/8.0.12/page_protocol_connection_phase_packets_protocol_handshake_response.html
// https://dev.mysql.com/doc/internals/en/connection-phase-packets.html#packet-Protocol::SSLRequest

#[derive(Debug)]
pub struct SslRequest {
    pub max_packet_size: u32,
    pub collation: u8,
}

impl Encode<'_, Capabilities> for SslRequest {
    fn encode_with(&self, buf: &mut Vec<u8>, capabilities: Capabilities) {
        buf.extend(&(capabilities.bits() as u32).to_le_bytes());
        buf.extend(&self.max_packet_size.to_le_bytes());
        buf.push(self.collation);

        // reserved: string<19>
        buf.extend(&[0_u8; 19]);

        if capabilities.contains(Capabilities::MYSQL) {
            // reserved: string<4>
            buf.extend(&[0_u8; 4]);
        } else {
            // extended client capabilities (MariaDB-specified): int<4>
            buf.extend(&((capabilities.bits() >> 32) as u32).to_le_bytes());
        }
    }
}
