//! Secret storage on this computer: the operating system credential store
//! (macOS Keychain, Windows Credential Manager, Linux Secret Service).

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;

const SERVICE: &str = "keyman";
const PROBE: &str = "probe";
const STORE_ERROR: &str = "Keyman cannot use the credential store of this computer.";

pub trait Store {
    fn get(&self, name: &str) -> Result<Option<String>, String>;
    fn set(&self, name: &str, value: &str) -> Result<(), String>;
    fn delete(&self, name: &str) -> Result<(), String>;

    /// False when this computer has no working credential store.
    fn available(&self) -> bool {
        true
    }
}

/// The credential store of the operating system. It tests the store one
/// time, so the answer cannot change during a command.
#[derive(Default)]
pub struct SystemStore(OnceCell<bool>);

impl SystemStore {
    pub fn new() -> SystemStore {
        SystemStore::default()
    }
}

fn entry(name: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, name).map_err(|_| STORE_ERROR.to_string())
}

impl Store for SystemStore {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        match entry(name)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(STORE_ERROR.to_string()),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        entry(name)?
            .set_password(value)
            .map_err(|_| STORE_ERROR.to_string())
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        match entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(STORE_ERROR.to_string()),
        }
    }

    fn available(&self) -> bool {
        *self.0.get_or_init(system_available)
    }
}

/// Test the credential store with a read of a fixed entry. "No entry" means
/// the store works. Any other error means it does not.
pub fn system_available() -> bool {
    match keyring::Entry::new(SERVICE, PROBE) {
        Ok(entry) => matches!(entry.get_password(), Ok(_) | Err(keyring::Error::NoEntry)),
        Err(_) => false,
    }
}

/// A store in memory, for tests.
#[derive(Default)]
pub struct MemoryStore(RefCell<HashMap<String, String>>);

impl MemoryStore {
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }
}

impl Store for MemoryStore {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self.0.borrow().get(name).cloned())
    }

    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.0
            .borrow_mut()
            .insert(name.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        self.0.borrow_mut().remove(name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trip() {
        let store = MemoryStore::new();
        assert_eq!(store.get("a").unwrap(), None);
        store.set("a", "1").unwrap();
        store.set("a", "2").unwrap();
        assert_eq!(store.get("a").unwrap(), Some("2".to_string()));
        store.delete("a").unwrap();
        store.delete("a").unwrap();
        assert_eq!(store.get("a").unwrap(), None);
        assert!(store.available());
    }
}
