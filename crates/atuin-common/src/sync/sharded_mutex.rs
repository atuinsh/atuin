use std::hash::{DefaultHasher, Hash, Hasher};
use std::marker::PhantomData;
use std::num::NonZeroUsize;

use tokio::sync::{Mutex, MutexGuard};

/// A fixed number of async mutexes, each guarding a `V`, selected by hashing a `K`.
#[derive(Debug)]
pub struct ShardedMutex<K, V> {
    shards: Box<[Mutex<V>]>,
    key: PhantomData<fn(&K)>,
}

impl<K: Hash, V: Default> ShardedMutex<K, V> {
    /// `shards` mutexes, each starting at `V::default()`.
    #[must_use]
    pub fn new(shards: NonZeroUsize) -> Self {
        Self {
            shards: (0..shards.get()).map(|_| Mutex::new(V::default())).collect(),
            key: PhantomData,
        }
    }
}

impl<K: Hash, V> ShardedMutex<K, V> {
    /// Lock the shard `key` hashes to, waiting while another task holds it.
    pub async fn lock(&self, key: &K) -> MutexGuard<'_, V> {
        self.shards[self.shard_of(key)].lock().await
    }

    /// How many shards there are.
    #[must_use]
    pub fn shards(&self) -> usize {
        self.shards.len()
    }

    fn shard_of(&self, key: &K) -> usize {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let count = u64::try_from(self.shards.len()).expect("a shard count fits in u64");
        usize::try_from(hasher.finish() % count).expect("a shard index is below the count")
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::Arc;
    use std::time::Duration;

    use rstest::rstest;

    use super::ShardedMutex;

    fn shards(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).expect("test shard counts are non-zero")
    }

    #[rstest]
    #[tokio::test]
    async fn the_same_key_waits_for_its_holder() {
        let mutex: Arc<ShardedMutex<&str, ()>> = Arc::new(ShardedMutex::new(shards(16)));
        let held = mutex.lock(&"key").await;

        let contender = Arc::clone(&mutex);
        let waiter = tokio::spawn(async move {
            let _ = contender.lock(&"key").await;
        });
        // While the guard is held, the second lock of the same key cannot complete.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), waiter_ready(&waiter)).await.is_err()
        );

        drop(held);
        waiter.await.expect("the waiter acquires the shard once it is released");
    }

    /// Resolves once `handle`'s task has finished; used to observe "still blocked".
    async fn waiter_ready(handle: &tokio::task::JoinHandle<()>) {
        while !handle.is_finished() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    #[rstest]
    #[tokio::test]
    async fn the_value_belongs_to_the_shard() {
        // One shard: every key shares it, and therefore shares its value.
        let mutex: ShardedMutex<u32, u32> = ShardedMutex::new(shards(1));
        *mutex.lock(&1).await += 1;
        *mutex.lock(&2).await += 1;
        assert_eq!(*mutex.lock(&3).await, 2);
    }

    #[rstest]
    #[tokio::test]
    async fn the_guard_hands_out_the_stored_value() {
        let mutex: ShardedMutex<String, Vec<u8>> = ShardedMutex::new(shards(8));
        mutex.lock(&"a".to_string()).await.push(7);
        assert_eq!(*mutex.lock(&"a".to_string()).await, vec![7]);
    }

    #[rstest]
    fn keys_spread_across_shards() {
        let mutex: ShardedMutex<u64, ()> = ShardedMutex::new(shards(64));
        let used: std::collections::HashSet<usize> =
            (0..256u64).map(|k| mutex.shard_of(&k)).collect();
        assert_eq!(mutex.shards(), 64);
        assert!(
            used.len() >= 32,
            "256 keys should land in at least half of 64 shards, got {}",
            used.len()
        );
    }

    #[rstest]
    #[tokio::test]
    async fn colliding_keys_serialise() {
        // One shard: two different keys collide, so the second waits for the first.
        let mutex: Arc<ShardedMutex<u32, ()>> = Arc::new(ShardedMutex::new(shards(1)));
        let held = mutex.lock(&1).await;

        let contender = Arc::clone(&mutex);
        let waiter = tokio::spawn(async move {
            let _ = contender.lock(&2).await;
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), waiter_ready(&waiter)).await.is_err()
        );

        drop(held);
        waiter.await.expect("the waiter acquires the shard once it is released");
    }

    #[rstest]
    #[tokio::test]
    async fn keys_on_different_shards_lock_independently() {
        let mutex: ShardedMutex<u64, ()> = ShardedMutex::new(shards(64));
        let first = 0u64;
        let other = (1..=1024u64)
            .find(|k| mutex.shard_of(k) != mutex.shard_of(&first))
            .expect("one of 1024 keys lands on another of 64 shards");
        let _held = mutex.lock(&first).await;

        // Not one big lock: a key on another shard is acquired immediately.
        assert!(tokio::time::timeout(Duration::from_millis(50), mutex.lock(&other)).await.is_ok());
    }

    #[rstest]
    fn is_send_and_sync_regardless_of_the_key() {
        fn assert_send_sync<T: Send + Sync>() {}
        // `Rc` is neither; the key marker must not drag the key's auto traits into the mutex.
        assert_send_sync::<ShardedMutex<std::rc::Rc<u8>, ()>>();
    }
}
