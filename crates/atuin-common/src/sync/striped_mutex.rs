use std::hash::{DefaultHasher, Hash, Hasher};
use std::marker::PhantomData;
use std::num::NonZeroUsize;

use tokio::sync::{Mutex, MutexGuard};

/// A fixed number of async mutexes, each guarding a `V`, selected by hashing a `K`.
///
/// Locking a key locks the stripe its hash lands in. Two keys may share a stripe, which costs a
/// little unnecessary waiting, never correctness -- as long as a task never holds two stripes at
/// once, since two keys in the same stripe would deadlock it against itself.
///
/// The `V` belongs to the stripe, not the key: keys that share a stripe see the same value. Use
/// `()` for pure mutual exclusion, which is what makes a per-key "check, then act" atomic without
/// allocating a mutex per key.
#[derive(Debug)]
pub struct StripedMutex<K, V> {
    stripes: Box<[Mutex<V>]>,
    key: PhantomData<fn(&K)>,
}

impl<K: Hash, V: Default> StripedMutex<K, V> {
    /// `stripes` mutexes, each starting at `V::default()`.
    #[must_use]
    pub fn new(stripes: NonZeroUsize) -> Self {
        Self {
            stripes: (0..stripes.get()).map(|_| Mutex::new(V::default())).collect(),
            key: PhantomData,
        }
    }
}

impl<K: Hash, V> StripedMutex<K, V> {
    /// Lock the stripe `key` hashes to, waiting while another task holds it.
    pub async fn lock(&self, key: &K) -> MutexGuard<'_, V> {
        self.stripes[self.stripe_of(key)].lock().await
    }

    /// How many stripes there are.
    #[must_use]
    pub fn stripes(&self) -> usize {
        self.stripes.len()
    }

    fn stripe_of(&self, key: &K) -> usize {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let count = u64::try_from(self.stripes.len()).expect("a stripe count fits in u64");
        usize::try_from(hasher.finish() % count).expect("a stripe index is below the count")
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::Arc;
    use std::time::Duration;

    use rstest::rstest;

    use super::StripedMutex;

    fn stripes(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).expect("test stripe counts are non-zero")
    }

    #[rstest]
    #[tokio::test]
    async fn the_same_key_waits_for_its_holder() {
        let mutex: Arc<StripedMutex<&str, ()>> = Arc::new(StripedMutex::new(stripes(16)));
        let held = mutex.lock(&"key").await;

        let contender = Arc::clone(&mutex);
        let waiter = tokio::spawn(async move {
            let _ = contender.lock(&"key").await;
        });
        // While the guard is held, the second lock of the same key cannot complete.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut Box::pin(waiter_ready(&waiter)))
                .await
                .is_err()
        );

        drop(held);
        waiter.await.expect("the waiter acquires the stripe once it is released");
    }

    /// Resolves once `handle`'s task has finished; used to observe "still blocked".
    async fn waiter_ready(handle: &tokio::task::JoinHandle<()>) {
        while !handle.is_finished() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    #[rstest]
    #[tokio::test]
    async fn the_value_belongs_to_the_stripe() {
        // One stripe: every key shares it, and therefore shares its value.
        let mutex: StripedMutex<u32, u32> = StripedMutex::new(stripes(1));
        *mutex.lock(&1).await += 1;
        *mutex.lock(&2).await += 1;
        assert_eq!(*mutex.lock(&3).await, 2);
    }

    #[rstest]
    #[tokio::test]
    async fn the_guard_hands_out_the_stored_value() {
        let mutex: StripedMutex<String, Vec<u8>> = StripedMutex::new(stripes(8));
        mutex.lock(&"a".to_string()).await.push(7);
        assert_eq!(*mutex.lock(&"a".to_string()).await, vec![7]);
    }

    #[rstest]
    fn keys_spread_across_stripes() {
        let mutex: StripedMutex<u64, ()> = StripedMutex::new(stripes(64));
        let used: std::collections::HashSet<usize> =
            (0..256u64).map(|k| mutex.stripe_of(&k)).collect();
        assert_eq!(mutex.stripes(), 64);
        assert!(
            used.len() >= 32,
            "256 keys should land in at least half of 64 stripes, got {}",
            used.len()
        );
    }
}
