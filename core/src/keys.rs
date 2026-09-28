//! Private key discovery and loading (OpenSSH, PEM/PKCS#8 and PuTTY `.ppk` formats).

use std::path::{Path, PathBuf};

use russh::keys::{HashAlg, PrivateKey, PublicKey};
use serde::Serialize;
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::util;

/// Default identities tried in automatic mode, in order (like OpenSSH, minus
/// hardware-backed and deprecated types).
const DEFAULT_IDENTITIES: &[&str] = &["id_ed25519", "id_ecdsa", "id_rsa"];

/// Metadata about a private key found on disk, for pickers in the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInfo {
    /// Path with the home directory shortened to `~`.
    pub path: String,
    pub key_type: String,
    pub encrypted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

/// Why a key could not be unlocked.
#[derive(Debug)]
pub enum UnlockError {
    NeedsPassphrase,
    WrongPassphrase,
    Invalid(String),
}

/// A private key file read into memory (contents are zeroized on drop).
pub struct KeyFile {
    pub path: PathBuf,
    text: Zeroizing<String>,
    /// Public half, when it can be known without the passphrase (OpenSSH format keys
    /// store it in clear; otherwise a sibling `.pub` file is used).
    pub public: Option<PublicKey>,
    pub encrypted: bool,
}

impl KeyFile {
    pub fn read(path: &Path) -> Result<KeyFile> {
        let text = Zeroizing::new(std::fs::read_to_string(path).map_err(|e| {
            Error::invalid(format!(
                "cannot read key {}: {e}",
                util::contract_tilde(path)
            ))
        })?);
        let trimmed = text.trim_start();
        let (public, encrypted) = if trimmed.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----") {
            let key = PrivateKey::from_openssh(trimmed).map_err(|e| {
                Error::invalid(format!("invalid key {}: {e}", util::contract_tilde(path)))
            })?;
            (Some(key.public_key().clone()), key.is_encrypted())
        } else if trimmed.starts_with("PuTTY-User-Key-File-") {
            let encrypted = !trimmed.contains("Encryption: none");
            let public = if encrypted {
                sibling_public_key(path)
            } else {
                russh::keys::decode_secret_key(trimmed, None)
                    .ok()
                    .map(|k| k.public_key().clone())
            };
            (public, encrypted)
        } else {
            match russh::keys::decode_secret_key(trimmed, None) {
                Ok(key) => (Some(key.public_key().clone()), false),
                Err(russh::keys::Error::KeyIsEncrypted) => (sibling_public_key(path), true),
                Err(e) => {
                    return Err(Error::invalid(format!(
                        "unsupported key {}: {e}",
                        util::contract_tilde(path)
                    )));
                }
            }
        };
        Ok(KeyFile {
            path: path.to_path_buf(),
            text,
            public,
            encrypted,
        })
    }

    /// Decodes the private key, decrypting it with `passphrase` if needed.
    pub fn unlock(&self, passphrase: Option<&str>) -> std::result::Result<PrivateKey, UnlockError> {
        if self.encrypted && passphrase.is_none() {
            return Err(UnlockError::NeedsPassphrase);
        }
        match russh::keys::decode_secret_key(self.text.trim_start(), passphrase) {
            Ok(key) => Ok(key),
            Err(russh::keys::Error::KeyIsEncrypted) => Err(UnlockError::NeedsPassphrase),
            Err(_) if self.encrypted => Err(UnlockError::WrongPassphrase),
            Err(e) => Err(UnlockError::Invalid(e.to_string())),
        }
    }

    pub fn display_path(&self) -> String {
        util::contract_tilde(&self.path)
    }
}

fn sibling_public_key(path: &Path) -> Option<PublicKey> {
    let mut pub_path = path.as_os_str().to_owned();
    pub_path.push(".pub");
    let text = std::fs::read_to_string(PathBuf::from(pub_path)).ok()?;
    PublicKey::from_openssh(text.trim()).ok()
}

/// Existing default identity files (`~/.ssh/id_ed25519`, ...).
pub fn default_identity_files() -> Vec<PathBuf> {
    let Some(dir) = util::ssh_dir() else {
        return Vec::new();
    };
    DEFAULT_IDENTITIES
        .iter()
        .map(|name| dir.join(name))
        .filter(|p| p.is_file())
        .collect()
}

fn looks_like_private_key(path: &Path) -> bool {
    use std::io::Read;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name.ends_with(".pub")
        || name.starts_with("known_hosts")
        || name.starts_with("authorized_keys")
        || name == "config"
    {
        return false;
    }
    let mut head = [0u8; 64];
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let n = file.read(&mut head).unwrap_or(0);
    let head = String::from_utf8_lossy(&head[..n]);
    (head.starts_with("-----BEGIN ") && head.contains("PRIVATE KEY"))
        || head.starts_with("PuTTY-User-Key-File-")
}

/// Private keys found in `~/.ssh`, sorted by name.
pub fn list_local_keys() -> Vec<KeyInfo> {
    let Some(dir) = util::ssh_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.metadata().map(|m| m.len() < 64 * 1024).unwrap_or(false))
        .filter(|p| looks_like_private_key(p))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|p| {
            let key = KeyFile::read(&p).ok()?;
            let (key_type, fingerprint) = match &key.public {
                Some(public) => (
                    public.algorithm().to_string(),
                    Some(public.fingerprint(HashAlg::Sha256).to_string()),
                ),
                None => ("private key".to_string(), None),
            };
            Some(KeyInfo {
                path: key.display_path(),
                key_type,
                encrypted: key.encrypted,
                fingerprint,
            })
        })
        .collect()
}
