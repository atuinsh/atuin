use atuin_common::record::{EncryptedData, Host, HostId, Record};
use atuin_common::utils::uuid_v7;
use rand::Rng;
use rand::distributions::Alphanumeric;

use crate::_util::context::BenchCtx;

pub struct BenchRecord;

impl BenchRecord {
    /// Controls how large the record payload is. Roughly, this is between 200 and 400 bytes for
    /// a typical history record.
    ///
    /// Breakdown:
    ///  - id (UUID string, 36 bytes)
    ///  - timestamp (u64, 8 bytes)
    ///  - duration (i64, 8 bytes)
    ///  - exit code (i64, 8 bytes)
    ///  - command (variable — average shell command is ~20-50 bytes, but can be much longer)
    ///  - cwd (path string, ~20-60 bytes)
    ///  - session (string, ~36 bytes)
    ///  - hostname (string, ~10-30 bytes)
    ///  - deleted_at (optional u64)
    ///  - author (string)
    const PAYLOAD_SIZE: usize = 300;

    /// Rough size of the PASETO PIE-wrapped key.
    const KEY_SIZE: usize = 150;

    /// Build a chain of `n` records sharing a single host and tag, with `idx` running `0..n`.
    ///
    /// The payloads are random bytes rather than real PASETO ciphertext. Nothing on either the
    /// upload or download path decrypts a record, so this is indistinguishable to the code under
    /// test while being far cheaper to produce.
    pub fn chain(ctx: &mut BenchCtx, n: usize) -> Vec<Record<EncryptedData>> {
        let host = Host::new(HostId(uuid_v7()));
        let version: String = "v1".into();
        let tag = uuid_v7().simple().to_string();
        let data: String = ctx
            .rng()
            .sample_iter(&Alphanumeric)
            .take(Self::PAYLOAD_SIZE)
            .map(char::from)
            .collect();
        let key: String = ctx
            .rng()
            .sample_iter(&Alphanumeric)
            .take(Self::KEY_SIZE)
            .map(char::from)
            .collect();

        (0..n as u64)
            .map(|idx| {
                Record::builder()
                    .host(host.clone())
                    .version(version.clone())
                    .tag(tag.clone())
                    .data(EncryptedData {
                        data: data.clone(),
                        content_encryption_key: key.clone(),
                    })
                    .idx(idx)
                    .build()
            })
            .collect()
    }
}
