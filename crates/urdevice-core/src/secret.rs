//! Secure storage for the identity key and the trust-store MAC key (R1.2).
//!
//! The core never touches the OS keystore itself. Desktop adapters in
//! `urdevice-platform` implement [`SecretStore`] with the keychain, DPAPI, or
//! Secret Service; Android implements it in Kotlin with a Keystore-wrapped key.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::{CoreError, Result};

/// Name of the identity private key (PKCS#8, Ed25519).
pub const IDENTITY_KEY: &str = "identity-key";
/// Name of the secret that keys the trust-store row MACs.
pub const STORE_MAC_KEY: &str = "store-mac-key";

/// A small key-value store for secrets, backed by the platform keystore.
pub trait SecretStore: Send + Sync {
    /// Read a secret. `Ok(None)` when it does not exist.
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>>;
    /// Create or replace a secret.
    fn set(&self, name: &str, value: &[u8]) -> Result<()>;
    /// Delete a secret. Deleting a missing secret is not an error.
    fn delete(&self, name: &str) -> Result<()>;
}

/// In-memory store for tests. Secrets are lost when it is dropped.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    items: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemorySecretStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let items = self
            .items
            .lock()
            .map_err(|_| CoreError::Secret("poisoned".into()))?;
        Ok(items.get(name).cloned())
    }

    fn set(&self, name: &str, value: &[u8]) -> Result<()> {
        let mut items = self
            .items
            .lock()
            .map_err(|_| CoreError::Secret("poisoned".into()))?;
        items.insert(name.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<()> {
        let mut items = self
            .items
            .lock()
            .map_err(|_| CoreError::Secret("poisoned".into()))?;
        items.remove(name);
        Ok(())
    }
}

/// Read a secret, or create it with `len` random bytes when it is missing.
pub fn get_or_create_random(store: &dyn SecretStore, name: &str, len: usize) -> Result<Vec<u8>> {
    if let Some(v) = store.get(name)?
        && v.len() == len
    {
        return Ok(v);
    }
    let v = crate::crypto::random_bytes(len)?;
    store.set(name, &v)?;
    Ok(v)
}
