use crate::client;
use crate::error::TLSError;
use crate::key;
use crate::msgs::enums::SignatureScheme;
use crate::sign;

use std::collections;
use std::sync::{Arc, Mutex};

/// An implementor of `StoresClientSessions` which does nothing.
pub struct NoClientSessionStorage {}

impl client::StoresClientSessions for NoClientSessionStorage {
    fn put(&self, _key: Vec<u8>, _value: Vec<u8>) -> bool {
        false
    }

    fn get(&self, _key: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

/// An implementor of `StoresClientSessions` that stores everything
/// in memory.  It enforces a limit on the number of entries
/// to bound memory usage.
pub struct ClientSessionMemoryCache {
    cache: Mutex<collections::HashMap<Vec<u8>, Vec<u8>>>,
    max_entries: usize,
}

impl ClientSessionMemoryCache {
    /// Make a new ClientSessionMemoryCache.  `size` is the
    /// maximum number of stored sessions.
    pub fn new(size: usize) -> Arc<ClientSessionMemoryCache> {
        debug_assert!(size > 0);
        Arc::new(ClientSessionMemoryCache {
            cache: Mutex::new(collections::HashMap::new()),
            max_entries: size,
        })
    }

    fn limit_size(&self) {
        let mut cache = self.cache.lock().unwrap();
        while cache.len() > self.max_entries {
            let k = cache.keys().next().unwrap().clone();
            cache.remove(&k);
        }
    }
}

impl client::StoresClientSessions for ClientSessionMemoryCache {
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> bool {
        self.cache
            .lock()
            .unwrap()
            .insert(key, value);
        self.limit_size();
        true
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.cache
            .lock()
            .unwrap()
            .get(key)
            .cloned()
    }
}

pub struct FailResolveClientCert {}

impl client::ResolvesClientCert for FailResolveClientCert {
    fn resolve(
        &self,
        _acceptable_issuers: &[&[u8]],
        _sigschemes: &[SignatureScheme],
    ) -> Option<sign::CertifiedKey> {
        None
    }

    fn has_certs(&self) -> bool {
        false
    }
}

pub struct AlwaysResolvesClientCert(sign::CertifiedKey);

impl AlwaysResolvesClientCert {
    pub fn new(
        chain: Vec<key::Certificate>,
        priv_key: &key::PrivateKey,
    ) -> Result<AlwaysResolvesClientCert, TLSError> {
        let key = sign::any_supported_type(priv_key)
            .map_err(|_| TLSError::General("invalid private key".into()))?;
        Ok(AlwaysResolvesClientCert(sign::CertifiedKey::new(
            chain,
            Arc::new(key),
        )))
    }
}

impl client::ResolvesClientCert for AlwaysResolvesClientCert {
    fn resolve(
        &self,
        _acceptable_issuers: &[&[u8]],
        _sigschemes: &[SignatureScheme],
    ) -> Option<sign::CertifiedKey> {
        Some(self.0.clone())
    }

    fn has_certs(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::StoresClientSessions;

    #[test]
    fn test_noclientsessionstorage_drops_put() {
        let c = NoClientSessionStorage {};
        assert_eq!(c.put(vec![0x01], vec![0x02]), false);
    }

    #[test]
    fn test_noclientsessionstorage_denies_gets() {
        let c = NoClientSessionStorage {};
        c.put(vec![0x01], vec![0x02]);
        assert_eq!(c.get(&[]), None);
        assert_eq!(c.get(&[0x01]), None);
        assert_eq!(c.get(&[0x02]), None);
    }

    #[test]
    fn test_clientsessionmemorycache_accepts_put() {
        let c = ClientSessionMemoryCache::new(4);
        assert_eq!(c.put(vec![0x01], vec![0x02]), true);
    }

    #[test]
    fn test_clientsessionmemorycache_persists_put() {
        let c = ClientSessionMemoryCache::new(4);
        assert_eq!(c.put(vec![0x01], vec![0x02]), true);
        assert_eq!(c.get(&[0x01]), Some(vec![0x02]));
        assert_eq!(c.get(&[0x01]), Some(vec![0x02]));
    }

    #[test]
    fn test_clientsessionmemorycache_overwrites_put() {
        let c = ClientSessionMemoryCache::new(4);
        assert_eq!(c.put(vec![0x01], vec![0x02]), true);
        assert_eq!(c.put(vec![0x01], vec![0x04]), true);
        assert_eq!(c.get(&[0x01]), Some(vec![0x04]));
    }

    #[test]
    fn test_clientsessionmemorycache_drops_to_maintain_size_invariant() {
        let c = ClientSessionMemoryCache::new(4);
        assert_eq!(c.put(vec![0x01], vec![0x02]), true);
        assert_eq!(c.put(vec![0x03], vec![0x04]), true);
        assert_eq!(c.put(vec![0x05], vec![0x06]), true);
        assert_eq!(c.put(vec![0x07], vec![0x08]), true);
        assert_eq!(c.put(vec![0x09], vec![0x0a]), true);

        let mut count = 0;
        if c.get(&[0x01]).is_some() {
            count += 1;
        }
        if c.get(&[0x03]).is_some() {
            count += 1;
        }
        if c.get(&[0x05]).is_some() {
            count += 1;
        }
        if c.get(&[0x07]).is_some() {
            count += 1;
        }
        if c.get(&[0x09]).is_some() {
            count += 1;
        }

        assert_eq!(count, 4);
    }
}
