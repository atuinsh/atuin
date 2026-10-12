//! Decoding records across cores.
//!
//! Decrypting and decoding a record is CPU-bound and independent of every other record, so a
//! page of them can be decoded on several threads at once: this is where a replay of a large
//! record store spends the time it does not spend writing.

use std::num::NonZeroUsize;
use std::sync::Arc;

use atuin_common::encryption::paseto_v4::EncryptedData;
use atuin_domain::record::Record;

/// Fewer records than this to a thread is not worth the hand-off.
const MIN_CHUNK: usize = 64;

/// Decode each of `records` with `decode` on the blocking thread pool, split across the
/// machine's cores, and return the results in the records' order.
///
/// `decode` is whatever a record type needs: typically decrypting the record with the store's
/// key and deserializing its payload, with any failure folded into `T`. It runs in the caller's
/// tracing span, and a panic in it is raised here, as if it had run inline.
pub async fn decode_parallel<T, F>(records: Vec<Record<EncryptedData>>, decode: F) -> Vec<T>
where
    T: Send + 'static,
    F: Fn(Record<EncryptedData>) -> T + Send + Sync + 'static,
{
    let threads = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    let chunk = records.len().div_ceil(threads).max(MIN_CHUNK);
    let decode = Arc::new(decode);

    let mut records = records.into_iter();
    let mut tasks = Vec::new();
    loop {
        let part: Vec<_> = records.by_ref().take(chunk).collect();
        if part.is_empty() {
            break;
        }
        let decode = Arc::clone(&decode);
        let span = tracing::Span::current();
        tasks.push(tokio::task::spawn_blocking(move || {
            let _span = span.enter();
            part.into_iter().map(|record| decode(record)).collect::<Vec<T>>()
        }));
    }

    let mut decoded = Vec::new();
    for task in tasks {
        match task.await {
            Ok(part) => decoded.extend(part),
            Err(err) => match err.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                // Only a runtime shutting down cancels a blocking task, and nothing is left to
                // decode for then.
                Err(err) => panic!("decoding records was cancelled: {err}"),
            },
        }
    }
    decoded
}

#[cfg(test)]
mod tests {
    use atuin_common::encryption::paseto_v4::Key;
    use atuin_domain::record::{DecryptedData, Host, HostId, RecordTag, RecordVersion};
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    fn records(key: &Key, payloads: &[Vec<u8>]) -> Vec<Record<EncryptedData>> {
        let host = Host::new(HostId(atuin_common::utils::uuid_v7()));
        payloads
            .iter()
            .enumerate()
            .map(|(idx, data)| {
                Record::builder()
                    .host(host.clone())
                    .version(RecordVersion::V1)
                    .tag(RecordTag::AiSession)
                    .idx(idx as u64)
                    .data(DecryptedData(data.clone()))
                    .build()
                    .encrypt(key)
            })
            .collect()
    }

    /// Whatever the page's size against the chunks, every record is decoded once, in order.
    #[rstest]
    fn decodes_every_record_in_order() {
        let pages = prop::collection::vec(prop::collection::vec(any::<u8>(), 0..16), 0..600);
        proptest!(ProptestConfig::with_cases(32), |(payloads in pages)| {
            let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
            let key = Key::generate();
            let page = records(&key, &payloads);
            let decoded = runtime.block_on(decode_parallel(page, move |record| {
                record.decrypt(&key).unwrap().data.0
            }));
            prop_assert_eq!(decoded, payloads);
        });
    }

    #[rstest]
    #[tokio::test]
    #[should_panic(expected = "bad record")]
    async fn a_panic_in_decode_is_raised_to_the_caller() {
        let key = Key::generate();
        let page = records(&key, &vec![vec![0]; 3 * MIN_CHUNK]);
        decode_parallel(page, |record| {
            assert!(record.idx != 2 * MIN_CHUNK as u64, "bad record");
        })
        .await;
    }
}
