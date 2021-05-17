use crate::cipher::{MessageDecrypter, MessageEncrypter};
use crate::error::TLSError;
use crate::msgs::message::{BorrowMessage, Message};

static SEQ_SOFT_LIMIT: u64 = 0xffff_ffff_ffff_0000u64;
static SEQ_HARD_LIMIT: u64 = 0xffff_ffff_ffff_fffeu64;

#[derive(PartialEq)]
enum DirectionState {
    /// No keying material.
    Invalid,

    /// Keying material present, but not yet in use.
    Prepared,

    /// Keying material in use.
    Active,
}

pub struct RecordLayer {
    message_encrypter: Box<dyn MessageEncrypter>,
    message_decrypter: Box<dyn MessageDecrypter>,
    write_seq: u64,
    read_seq: u64,
    encrypt_state: DirectionState,
    decrypt_state: DirectionState,
}

impl RecordLayer {
    pub fn new() -> RecordLayer {
        RecordLayer {
            message_encrypter: MessageEncrypter::invalid(),
            message_decrypter: MessageDecrypter::invalid(),
            write_seq: 0,
            read_seq: 0,
            encrypt_state: DirectionState::Invalid,
            decrypt_state: DirectionState::Invalid,
        }
    }

    pub fn is_encrypting(&self) -> bool {
        self.encrypt_state == DirectionState::Active
    }

    pub fn is_decrypting(&self) -> bool {
        self.decrypt_state == DirectionState::Active
    }

    /// Prepare to use the given `MessageEncrypter` for future message encryption.
    /// It is not used until you call `start_encrypting`.
    pub fn prepare_message_encrypter(&mut self, cipher: Box<dyn MessageEncrypter>) {
        self.message_encrypter = cipher;
        self.write_seq = 0;
        self.encrypt_state = DirectionState::Prepared;
    }

    /// Prepare to use the given `MessageDecrypter` for future message decryption.
    /// It is not used until you call `start_decrypting`.
    pub fn prepare_message_decrypter(&mut self, cipher: Box<dyn MessageDecrypter>) {
        self.message_decrypter = cipher;
        self.read_seq = 0;
        self.decrypt_state = DirectionState::Prepared;
    }

    /// Start using the `MessageEncrypter` previously provided to the previous
    /// call to `prepare_message_encrypter`.
    pub fn start_encrypting(&mut self) {
        debug_assert!(self.encrypt_state == DirectionState::Prepared);
        self.encrypt_state = DirectionState::Active;
    }

    /// Start using the `MessageDecrypter` previously provided to the previous
    /// call to `prepare_message_decrypter`.
    pub fn start_decrypting(&mut self) {
        debug_assert!(self.decrypt_state == DirectionState::Prepared);
        self.decrypt_state = DirectionState::Active;
    }

    /// Set and start using the given `MessageEncrypter` for future outgoing
    /// message encryption.
    pub fn set_message_encrypter(&mut self, cipher: Box<dyn MessageEncrypter>) {
        self.prepare_message_encrypter(cipher);
        self.start_encrypting();
    }

    /// Set and start using the given `MessageDecrypter` for future incoming
    /// message decryption.
    pub fn set_message_decrypter(&mut self, cipher: Box<dyn MessageDecrypter>) {
        self.prepare_message_decrypter(cipher);
        self.start_decrypting();
    }

    /// Return true if the peer appears to getting close to encrypting
    /// too many messages with this key.
    ///
    /// Perhaps if we send an alert well before their counter wraps, a
    /// buggy peer won't make a terrible mistake here?
    ///
    /// Note that there's no reason to refuse to decrypt: the security
    /// failure has already happened.
    pub fn wants_close_before_decrypt(&self) -> bool {
        self.read_seq == SEQ_SOFT_LIMIT
    }

    /// Return true if we are getting close to encrypting too many
    /// messages with our encryption key.
    pub fn wants_close_before_encrypt(&self) -> bool {
        self.write_seq == SEQ_SOFT_LIMIT
    }

    /// Return true if we outright refuse to do anything with the
    /// encryption key.
    pub fn encrypt_exhausted(&self) -> bool {
        self.write_seq >= SEQ_HARD_LIMIT
    }

    /// Decrypt a TLS message.
    ///
    /// `encr` is a decoded message allegedly received from the peer.
    /// If it can be decrypted, its decryption is returned.  Otherwise,
    /// an error is returned.
    pub fn decrypt_incoming(&mut self, encr: Message) -> Result<Message, TLSError> {
        debug_assert!(self.decrypt_state == DirectionState::Active);
        let seq = self.read_seq;
        self.read_seq += 1;
        self.message_decrypter
            .decrypt(encr, seq)
    }

    /// Encrypt a TLS message.
    ///
    /// `plain` is a TLS message we'd like to send.  This function
    /// panics if the requisite keying material hasn't been established yet.
    pub fn encrypt_outgoing(&mut self, plain: BorrowMessage) -> Message {
        debug_assert!(self.encrypt_state == DirectionState::Active);
        assert!(!self.encrypt_exhausted());
        let seq = self.write_seq;
        self.write_seq += 1;
        self.message_encrypter
            .encrypt(plain, seq)
            .unwrap()
    }
}
