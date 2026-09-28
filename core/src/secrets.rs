//! Passwords and key passphrases, stored only in the OS credential store:
//! Windows Credential Manager, macOS Keychain or the Secret Service on Linux.
//!
//! Nothing secret is ever written to NexSSH's own files. If no credential store is
//! available (e.g. a headless Linux box without a Secret Service), saving is disabled
//! and the user is simply asked for the secret when connecting.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use keyring_core::CredentialStore;
use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// Service name under which all NexSSH secrets are stored.
pub const SERVICE: &str = "NexSSH";

enum Backend {
    Os(Arc<CredentialStore>),
    Memory(Mutex<HashMap<String, String>>),
    Unavailable(String),
}

pub struct Secrets {
    backend: OnceLock<Backend>,
    memory: bool,
}

impl Secrets {
    /// Uses the platform credential store. Initialization is lazy so that a slow
    /// D-Bus/Keychain never delays application start-up.
    pub fn system() -> Self {
        Secrets {
            backend: OnceLock::new(),
            memory: false,
        }
    }

    /// Non-persistent store for tests.
    pub fn in_memory() -> Self {
        Secrets {
            backend: OnceLock::new(),
            memory: true,
        }
    }

    fn backend(&self) -> &Backend {
        self.backend.get_or_init(|| {
            if self.memory {
                return Backend::Memory(Mutex::new(HashMap::new()));
            }
            match platform_store() {
                Ok(store) => Backend::Os(store),
                Err(err) => {
                    log::warn!("OS credential store unavailable: {err}");
                    Backend::Unavailable(err.to_string())
                }
            }
        })
    }

    /// `None` when secrets can be stored, otherwise the reason why not.
    ///
    /// Connecting to the store is not enough (e.g. a Secret Service without a default
    /// collection), so this probes it with a lookup of an entry that never exists.
    pub fn unavailable_reason(&self) -> Option<String> {
        match self.backend() {
            Backend::Unavailable(reason) => Some(reason.clone()),
            Backend::Os(store) => match entry(store, "probe")
                .and_then(|e| e.get_password().map_err(|e| Error::Secret(e.to_string())))
            {
                Ok(_) => None,
                Err(Error::Secret(msg)) if msg == keyring_core::Error::NoEntry.to_string() => None,
                Err(e) => Some(e.to_string()),
            },
            Backend::Memory(_) => None,
        }
    }

    pub fn is_available(&self) -> bool {
        self.unavailable_reason().is_none()
    }

    /// Account name for a saved server's password.
    pub fn password_account(server_id: &str) -> String {
        format!("password:{server_id}")
    }

    /// Account name for the password of a saved server's jump host, when that jump host is
    /// typed in place (`host[:port]`) rather than another saved server.
    pub fn jump_password_account(server_id: &str) -> String {
        format!("jump-password:{server_id}")
    }

    /// Account name for a private key passphrase (keyed by the key's path, so the
    /// passphrase is shared by every server using that key).
    pub fn passphrase_account(key_path: &Path) -> String {
        let canonical = std::fs::canonicalize(key_path).unwrap_or_else(|_| key_path.to_path_buf());
        format!("passphrase:{}", canonical.display())
    }

    pub fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>> {
        match self.backend() {
            Backend::Os(store) => match entry(store, account)?.get_password() {
                Ok(secret) => Ok(Some(Zeroizing::new(secret))),
                Err(keyring_core::Error::NoEntry) => Ok(None),
                Err(err) => Err(Error::Secret(err.to_string())),
            },
            Backend::Memory(map) => Ok(map
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(account)
                .cloned()
                .map(Zeroizing::new)),
            Backend::Unavailable(_) => Ok(None),
        }
    }

    pub fn set(&self, account: &str, secret: &str) -> Result<()> {
        match self.backend() {
            Backend::Os(store) => entry(store, account)?
                .set_password(secret)
                .map_err(|e| Error::Secret(e.to_string())),
            Backend::Memory(map) => {
                map.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(account.to_string(), secret.to_string());
                Ok(())
            }
            Backend::Unavailable(reason) => Err(Error::Secret(reason.clone())),
        }
    }

    /// Deletes a secret; succeeds if it did not exist.
    pub fn delete(&self, account: &str) -> Result<()> {
        match self.backend() {
            Backend::Os(store) => match entry(store, account)?.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
                Err(err) => Err(Error::Secret(err.to_string())),
            },
            Backend::Memory(map) => {
                map.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(account);
                Ok(())
            }
            Backend::Unavailable(_) => Ok(()),
        }
    }

    pub fn has(&self, account: &str) -> bool {
        matches!(self.get(account), Ok(Some(_)))
    }

    // Credential stores may block (unlock dialogs, D-Bus round trips): async callers
    // go through the blocking thread pool.

    pub async fn available_async(self: &Arc<Self>) -> bool {
        let this = Arc::clone(self);
        tokio::task::spawn_blocking(move || this.is_available())
            .await
            .unwrap_or(false)
    }

    pub async fn get_async(self: &Arc<Self>, account: String) -> Result<Option<Zeroizing<String>>> {
        let this = Arc::clone(self);
        tokio::task::spawn_blocking(move || this.get(&account))
            .await
            .map_err(|e| Error::Secret(e.to_string()))?
    }

    pub async fn set_async(
        self: &Arc<Self>,
        account: String,
        secret: Zeroizing<String>,
    ) -> Result<()> {
        let this = Arc::clone(self);
        tokio::task::spawn_blocking(move || this.set(&account, &secret))
            .await
            .map_err(|e| Error::Secret(e.to_string()))?
    }
}

fn entry(store: &Arc<CredentialStore>, account: &str) -> Result<keyring_core::Entry> {
    let label = format!("{SERVICE} {account}");
    let mut modifiers: HashMap<&str, &str> = HashMap::new();
    if cfg!(windows) {
        // Readable name in Credential Manager; "Local" keeps secrets on this machine
        // instead of roaming with the user profile.
        modifiers.insert("target", &label);
        modifiers.insert("persistence", "Local");
    } else if cfg!(all(unix, not(target_os = "macos"))) {
        modifiers.insert("label", &label);
    }
    let modifiers = (!modifiers.is_empty()).then_some(&modifiers);
    store
        .build(SERVICE, account, modifiers)
        .map_err(|e| Error::Secret(e.to_string()))
}

#[cfg(target_os = "windows")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(target_os = "macos")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(dbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    )
)))]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Err(keyring_core::Error::NotSupportedByStore(
        "no credential store for this platform".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_backend_roundtrip() {
        let s = Secrets::in_memory();
        assert!(s.is_available());
        let acc = Secrets::password_account("abc");
        assert_eq!(s.get(&acc).unwrap(), None);
        s.set(&acc, "hunter2").unwrap();
        assert_eq!(
            s.get(&acc).unwrap().as_deref().map(String::as_str),
            Some("hunter2")
        );
        assert!(s.has(&acc));
        s.delete(&acc).unwrap();
        s.delete(&acc).unwrap();
        assert!(!s.has(&acc));
    }
}
