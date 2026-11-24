//! Provider credentials: OS credential store with a session-only fallback.
//!
//! Keys are scoped by account name: `replicate`, `fal`, or
//! `custom:<origin>` so a token saved for one custom server is never sent to
//! another origin.

use crate::redact::Secret;
use std::collections::HashMap;
use std::sync::Mutex;

pub const SERVICE: &str = "app.threedmaker.desktop";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("the system credential store is unavailable: {0}")]
    Unavailable(String),
}

pub trait CredentialStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<Secret>, StoreError>;
    fn set(&self, account: &str, secret: &Secret) -> Result<(), StoreError>;
    fn delete(&self, account: &str) -> Result<(), StoreError>;
}

/// macOS Keychain, Windows Credential Manager, or Secret Service on Linux.
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
    fn entry(&self, account: &str) -> Result<keyring::Entry, StoreError> {
        keyring::Entry::new(&self.service, account)
            .map_err(|e| StoreError::Unavailable(e.to_string()))
    }
}

impl CredentialStore for KeyringStore {
    fn get(&self, account: &str) -> Result<Option<Secret>, StoreError> {
        match self.entry(account)?.get_password() {
            Ok(v) => Ok(Some(Secret::new(v))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(StoreError::Unavailable(e.to_string())),
        }
    }
    fn set(&self, account: &str, secret: &Secret) -> Result<(), StoreError> {
        self.entry(account)?
            .set_password(secret.expose())
            .map_err(|e| StoreError::Unavailable(e.to_string()))
    }
    fn delete(&self, account: &str) -> Result<(), StoreError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(StoreError::Unavailable(e.to_string())),
        }
    }
}

/// In-memory store: used for session-only keys and in tests.
#[derive(Default)]
pub struct MemoryStore {
    map: Mutex<HashMap<String, Secret>>,
    fail: bool,
}

impl MemoryStore {
    /// A store that always reports itself unavailable (tests the fallback path).
    pub fn failing() -> Self {
        Self {
            map: Mutex::default(),
            fail: true,
        }
    }
}

impl CredentialStore for MemoryStore {
    fn get(&self, account: &str) -> Result<Option<Secret>, StoreError> {
        if self.fail {
            return Err(StoreError::Unavailable("simulated".into()));
        }
        Ok(self.map.lock().unwrap().get(account).cloned())
    }
    fn set(&self, account: &str, secret: &Secret) -> Result<(), StoreError> {
        if self.fail {
            return Err(StoreError::Unavailable("simulated".into()));
        }
        self.map
            .lock()
            .unwrap()
            .insert(account.to_string(), secret.clone());
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<(), StoreError> {
        if self.fail {
            return Err(StoreError::Unavailable("simulated".into()));
        }
        self.map.lock().unwrap().remove(account);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Persistence {
    Stored,
    Session,
}

/// Persistent store + session overlay.
pub struct Credentials {
    persistent: Box<dyn CredentialStore>,
    session: MemoryStore,
}

impl Credentials {
    pub fn new(persistent: Box<dyn CredentialStore>) -> Self {
        Self {
            persistent,
            session: MemoryStore::default(),
        }
    }

    pub fn persistent_available(&self) -> bool {
        self.persistent.get("__probe__").is_ok()
    }

    pub fn get(&self, account: &str) -> Option<(Secret, Persistence)> {
        if let Ok(Some(s)) = self.session.get(account) {
            return Some((s, Persistence::Session));
        }
        match self.persistent.get(account) {
            Ok(Some(s)) => Some((s, Persistence::Stored)),
            _ => None,
        }
    }

    /// Saves to the OS store; on failure returns Err so the UI can offer session-only.
    pub fn save(
        &self,
        account: &str,
        secret: &Secret,
        session_only: bool,
    ) -> Result<Persistence, StoreError> {
        if session_only {
            let _ = self.session.set(account, secret);
            return Ok(Persistence::Session);
        }
        self.persistent.set(account, secret)?;
        let _ = self.session.delete(account);
        Ok(Persistence::Stored)
    }

    pub fn remove(&self, account: &str) {
        let _ = self.session.delete(account);
        let _ = self.persistent.delete(account);
    }
}

pub fn custom_account(origin: &str) -> String {
    format!("custom:{origin}")
}
