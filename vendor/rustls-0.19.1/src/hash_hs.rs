#[cfg(feature = "logging")]
use crate::log::warn;
use crate::msgs::codec::Codec;
use crate::msgs::handshake::HandshakeMessagePayload;
use crate::msgs::message::{Message, MessagePayload};
use ring::digest;
use std::mem;

/// This deals with keeping a running hash of the handshake
/// payloads.  This is computed by buffering initially.  Once
/// we know what hash function we need to use we switch to
/// incremental hashing.
///
/// For client auth, we also need to buffer all the messages.
/// This is disabled in cases where client auth is not possible.
pub struct HandshakeHash {
    /// None before we know what hash function we're using
    alg: Option<&'static digest::Algorithm>,

    /// None before we know what hash function we're using
    ctx: Option<digest::Context>,

    /// true if we need to keep all messages
    client_auth_enabled: bool,

    /// buffer for pre-hashing stage and client-auth.
    buffer: Vec<u8>,
}

impl HandshakeHash {
    pub fn new() -> HandshakeHash {
        HandshakeHash {
            alg: None,
            ctx: None,
            client_auth_enabled: false,
            buffer: Vec::new(),
        }
    }

    /// We might be doing client auth, so need to keep a full
    /// log of the handshake.
    pub fn set_client_auth_enabled(&mut self) {
        debug_assert!(self.ctx.is_none()); // or we might have already discarded messages
        self.client_auth_enabled = true;
    }

    /// We decided not to do client auth after all, so discard
    /// the transcript.
    pub fn abandon_client_auth(&mut self) {
        self.client_auth_enabled = false;
        self.buffer.drain(..);
    }

    /// We now know what hash function the verify_data will use.
    pub fn start_hash(&mut self, alg: &'static digest::Algorithm) -> bool {
        match self.alg {
            None => {}
            Some(started) => {
                if started != alg {
                    // hash type is changing
                    warn!("altered hash to HandshakeHash::start_hash");
                    return false;
                }

                return true;
            }
        }
        self.alg = Some(alg);
        debug_assert!(self.ctx.is_none());

        let mut ctx = digest::Context::new(alg);
        ctx.update(&self.buffer);
        self.ctx = Some(ctx);

        // Discard buffer if we don't need it now.
        if !self.client_auth_enabled {
            self.buffer.drain(..);
        }
        true
    }

    /// Hash/buffer a handshake message.
    pub fn add_message(&mut self, m: &Message) -> &mut HandshakeHash {
        match m.payload {
            MessagePayload::Handshake(ref hs) => {
                let buf = hs.get_encoding();
                self.update_raw(&buf);
            }
            _ => {}
        };
        self
    }

    /// Hash or buffer a byte slice.
    fn update_raw(&mut self, buf: &[u8]) -> &mut Self {
        if self.ctx.is_some() {
            self.ctx.as_mut().unwrap().update(buf);
        }

        if self.ctx.is_none() || self.client_auth_enabled {
            self.buffer.extend_from_slice(buf);
        }

        self
    }

    /// Get the hash value if we were to hash `extra` too,
    /// using hash function `hash`.
    pub fn get_hash_given(&self, hash: &'static digest::Algorithm, extra: &[u8]) -> Vec<u8> {
        let mut ctx = if self.ctx.is_none() {
            let mut ctx = digest::Context::new(hash);
            ctx.update(&self.buffer);
            ctx
        } else {
            self.ctx.as_ref().unwrap().clone()
        };

        ctx.update(extra);
        let hash = ctx.finish();
        let mut ret = Vec::new();
        ret.extend_from_slice(hash.as_ref());
        ret
    }

    /// Take the current hash value, and encapsulate it in a
    /// 'handshake_hash' handshake message.  Start this hash
    /// again, with that message at the front.
    pub fn rollup_for_hrr(&mut self) {
        let old_hash = self.ctx.take().unwrap().finish();
        let old_handshake_hash_msg =
            HandshakeMessagePayload::build_handshake_hash(old_hash.as_ref());

        self.ctx = Some(digest::Context::new(self.alg.unwrap()));
        self.update_raw(&old_handshake_hash_msg.get_encoding());
    }

    /// Get the current hash value.
    pub fn get_current_hash(&self) -> Vec<u8> {
        let hash = self
            .ctx
            .as_ref()
            .unwrap()
            .clone()
            .finish();
        let mut ret = Vec::new();
        ret.extend_from_slice(hash.as_ref());
        ret
    }

    /// Takes this object's buffer containing all handshake messages
    /// so far.  This method only works once; it resets the buffer
    /// to empty.
    pub fn take_handshake_buf(&mut self) -> Vec<u8> {
        debug_assert!(self.client_auth_enabled);
        mem::replace(&mut self.buffer, Vec::new())
    }
}

#[cfg(test)]
mod test {
    use super::HandshakeHash;
    use ring::digest;

    #[test]
    fn hashes_correctly() {
        let mut hh = HandshakeHash::new();
        hh.update_raw(b"hello");
        assert_eq!(hh.buffer.len(), 5);
        hh.start_hash(&digest::SHA256);
        assert_eq!(hh.buffer.len(), 0);
        hh.update_raw(b"world");
        let h = hh.get_current_hash();
        assert_eq!(h[0], 0x93);
        assert_eq!(h[1], 0x6a);
        assert_eq!(h[2], 0x18);
        assert_eq!(h[3], 0x5c);
    }

    #[test]
    fn buffers_correctly() {
        let mut hh = HandshakeHash::new();
        hh.set_client_auth_enabled();
        hh.update_raw(b"hello");
        assert_eq!(hh.buffer.len(), 5);
        hh.start_hash(&digest::SHA256);
        assert_eq!(hh.buffer.len(), 5);
        hh.update_raw(b"world");
        assert_eq!(hh.buffer.len(), 10);
        let h = hh.get_current_hash();
        assert_eq!(h[0], 0x93);
        assert_eq!(h[1], 0x6a);
        assert_eq!(h[2], 0x18);
        assert_eq!(h[3], 0x5c);
        let buf = hh.take_handshake_buf();
        assert_eq!(b"helloworld".to_vec(), buf);
    }

    #[test]
    fn abandon() {
        let mut hh = HandshakeHash::new();
        hh.set_client_auth_enabled();
        hh.update_raw(b"hello");
        assert_eq!(hh.buffer.len(), 5);
        hh.start_hash(&digest::SHA256);
        assert_eq!(hh.buffer.len(), 5);
        hh.abandon_client_auth();
        assert_eq!(hh.buffer.len(), 0);
        hh.update_raw(b"world");
        assert_eq!(hh.buffer.len(), 0);
        let h = hh.get_current_hash();
        assert_eq!(h[0], 0x93);
        assert_eq!(h[1], 0x6a);
        assert_eq!(h[2], 0x18);
        assert_eq!(h[3], 0x5c);
    }
}
