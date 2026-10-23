//! A backup of everything NexSSH keeps in one JSON file: servers and groups, the
//! application's settings, the host keys accepted in NexSSH and, when asked for, the saved
//! passwords.
//!
//! Passwords never leave the OS keychain in the clear: they go into the file encrypted
//! with a password of the backup (Argon2id, then AES-256-GCM). Without it a backup is
//! restored without passwords. Restoring replaces the servers; the host keys of the backup
//! are added to the ones known here, since forgetting one would only bring its question back.

use std::collections::{BTreeMap, HashSet};

use data_encoding::BASE64;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::secrets::Secrets;
use crate::store::StoreData;
use crate::{Core, i18n, util};

/// What the `format` field of a backup says.
pub const FORMAT: &str = "nexssh-backup";
const VERSION: u32 = 1;

const KDF: &str = "argon2id";
const CIPHER: &str = "aes-256-gcm";
const SALT_LEN: usize = 16;

/// The cost of turning the backup's password into a key: about a quarter of a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KdfCost {
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
}

const COST: KdfCost = KdfCost {
    memory_kib: 64 * 1024,
    iterations: 3,
    parallelism: 1,
};

/// A file may ask for more, up to these limits (a crafted one could ask for any amount of memory).
const MAX_COST: KdfCost = KdfCost {
    memory_kib: 1024 * 1024,
    iterations: 64,
    parallelism: 16,
};

/// The contents of a backup file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    pub format: String,
    pub version: u32,
    /// NexSSH's version that wrote the file.
    #[serde(default)]
    pub app_version: String,
    /// Seconds since the Unix epoch.
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub servers: StoreData,
    /// The front-end's settings object, as it stores it.
    #[serde(default)]
    pub settings: Value,
    /// NexSSH's own `known_hosts` file.
    #[serde(default)]
    pub known_hosts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passwords: Option<Sealed>,
}

/// Saved passwords, encrypted with the password of the backup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sealed {
    pub kdf: String,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
    pub salt: String,
    pub cipher: String,
    pub nonce: String,
    pub data: String,
}

/// What a backup holds, shown before it is restored.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub app_version: String,
    pub created: u64,
    pub servers: usize,
    pub groups: usize,
    /// Passwords are inside: restoring them takes the backup's password.
    pub passwords: bool,
}

/// A backup ready to be written, and how many passwords went into it.
pub struct Created {
    pub json: Vec<u8>,
    pub passwords: usize,
}

/// What restoring a backup did.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Restored {
    pub servers: usize,
    pub passwords: usize,
    /// Passwords the keychain did not take.
    pub passwords_failed: usize,
}

/// Account names of the passwords kept for the servers of `data`.
fn password_accounts(data: &StoreData) -> Vec<String> {
    data.servers
        .iter()
        .flat_map(|s| {
            [
                Secrets::password_account(&s.id),
                Secrets::jump_password_account(&s.id),
            ]
        })
        .collect()
}

/// Puts together a backup of `core` with the front-end's `settings`. With `password`, the
/// saved passwords of the servers go in too, encrypted with it.
pub fn create(core: &Core, settings: Value, password: Option<&str>) -> Result<Created> {
    create_with(core, settings, password, COST)
}

fn create_with(
    core: &Core,
    settings: Value,
    password: Option<&str>,
    cost: KdfCost,
) -> Result<Created> {
    let servers = core.store.snapshot();
    let known_hosts = match std::fs::read_to_string(core.known_hosts.app_file()) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let mut count = 0;
    let passwords = match password {
        Some(password) => {
            let mut found = BTreeMap::new();
            for account in password_accounts(&servers) {
                if let Some(secret) = core.secrets.get(&account)? {
                    found.insert(account, secret.as_str().to_owned());
                }
            }
            count = found.len();
            let plain = Zeroizing::new(serde_json::to_vec(&found)?);
            (count > 0)
                .then(|| seal(password, &plain, cost))
                .transpose()?
        }
        None => None,
    };
    let backup = Backup {
        format: FORMAT.to_string(),
        version: VERSION,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        created: util::now_secs(),
        servers,
        settings,
        known_hosts,
        passwords,
    };
    Ok(Created {
        json: serde_json::to_vec_pretty(&backup)?,
        passwords: count,
    })
}

/// Reads a backup file's contents.
pub fn parse(bytes: &[u8]) -> Result<Backup> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| Error::invalid(i18n::backup_not_ours()))?;
    if value.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(Error::invalid(i18n::backup_not_ours()));
    }
    let backup: Backup = serde_json::from_value(value)?;
    if backup.version > VERSION {
        return Err(Error::invalid(i18n::backup_too_new(&backup.app_version)));
    }
    Ok(backup)
}

impl Backup {
    pub fn summary(&self) -> Summary {
        Summary {
            app_version: self.app_version.clone(),
            created: self.created,
            servers: self.servers.servers.len(),
            groups: self.servers.groups.len(),
            passwords: self.passwords.is_some(),
        }
    }

    /// The saved passwords inside, by keychain account; an error when `password` is not the
    /// backup's.
    pub fn open_passwords(&self, password: &str) -> Result<BTreeMap<String, String>> {
        let Some(sealed) = &self.passwords else {
            return Ok(BTreeMap::new());
        };
        let plain = open(sealed, password)?;
        serde_json::from_slice(&plain).map_err(|_| Error::invalid(i18n::backup_damaged()))
    }
}

/// Replaces the servers of `core` with the backup's and adds its host keys. `passwords` (from
/// [`Backup::open_passwords`]) go into the keychain; the passwords kept for the servers that
/// were here are forgotten.
pub fn restore(
    core: &Core,
    backup: &Backup,
    passwords: &BTreeMap<String, String>,
) -> Result<Restored> {
    let before = core.store.snapshot();
    let after = core.store.replace(backup.servers.clone())?;
    for account in password_accounts(&before) {
        let _ = core.secrets.delete(&account);
    }
    add_known_hosts(core.known_hosts.app_file(), &backup.known_hosts)?;

    // Only the passwords of the servers restored: a file names keychain accounts, and must
    // not reach any other.
    let wanted: HashSet<String> = password_accounts(&after).into_iter().collect();
    let mut restored = Restored {
        servers: after.servers.len(),
        ..Restored::default()
    };
    for (account, secret) in passwords.iter().filter(|(a, _)| wanted.contains(*a)) {
        match core.secrets.set(account, secret) {
            Ok(()) => restored.passwords += 1,
            Err(e) => {
                log::warn!("password {account} not restored: {e}");
                restored.passwords_failed += 1;
            }
        }
    }
    Ok(restored)
}

/// Appends the lines of `text` that the file does not have yet.
fn add_known_hosts(path: &std::path::Path, text: &str) -> Result<()> {
    let current = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let have: HashSet<&str> = current.lines().map(str::trim).collect();
    let mut seen = HashSet::new();
    let new: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !have.contains(l) && seen.insert(*l))
        .collect();
    if new.is_empty() {
        return Ok(());
    }
    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    for line in new {
        next.push_str(line);
        next.push('\n');
    }
    Ok(util::atomic_write(path, next.as_bytes())?)
}

// ---- encryption ---------------------------------------------------------------------

fn derive_key(password: &str, salt: &[u8], cost: KdfCost) -> Result<Zeroizing<[u8; 32]>> {
    let params = argon2::Params::new(cost.memory_kib, cost.iterations, cost.parallelism, Some(32))
        .map_err(|e| Error::invalid(i18n::backup_encryption_failed(e)))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|e| Error::invalid(i18n::backup_encryption_failed(e)))?;
    Ok(key)
}

fn cipher(key: &[u8; 32]) -> Result<LessSafeKey> {
    let key = UnboundKey::new(&AES_256_GCM, key)
        .map_err(|_| Error::invalid(i18n::backup_encryption_failed("AES-256-GCM")))?;
    Ok(LessSafeKey::new(key))
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|e| Error::invalid(i18n::backup_encryption_failed(e)))?;
    Ok(bytes)
}

fn seal(password: &str, plain: &[u8], cost: KdfCost) -> Result<Sealed> {
    let salt: [u8; SALT_LEN] = random()?;
    let nonce: [u8; NONCE_LEN] = random()?;
    let key = cipher(&*derive_key(password, &salt, cost)?)?;
    let mut data = plain.to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(FORMAT.as_bytes()),
        &mut data,
    )
    .map_err(|_| Error::invalid(i18n::backup_encryption_failed("AES-256-GCM")))?;
    Ok(Sealed {
        kdf: KDF.to_string(),
        memory_kib: cost.memory_kib,
        iterations: cost.iterations,
        parallelism: cost.parallelism,
        salt: BASE64.encode(&salt),
        cipher: CIPHER.to_string(),
        nonce: BASE64.encode(&nonce),
        data: BASE64.encode(&data),
    })
}

fn open(sealed: &Sealed, password: &str) -> Result<Zeroizing<Vec<u8>>> {
    let damaged = || Error::invalid(i18n::backup_damaged());
    let cost = KdfCost {
        memory_kib: sealed.memory_kib,
        iterations: sealed.iterations,
        parallelism: sealed.parallelism,
    };
    if sealed.kdf != KDF
        || sealed.cipher != CIPHER
        || cost.memory_kib > MAX_COST.memory_kib
        || cost.iterations > MAX_COST.iterations
        || cost.parallelism > MAX_COST.parallelism
    {
        return Err(damaged());
    }
    let salt = BASE64
        .decode(sealed.salt.as_bytes())
        .map_err(|_| damaged())?;
    let nonce: [u8; NONCE_LEN] = BASE64
        .decode(sealed.nonce.as_bytes())
        .ok()
        .and_then(|n| n.try_into().ok())
        .ok_or_else(damaged)?;
    let mut data = Zeroizing::new(
        BASE64
            .decode(sealed.data.as_bytes())
            .map_err(|_| damaged())?,
    );
    let key = cipher(&*derive_key(password, &salt, cost)?)?;
    let len = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(FORMAT.as_bytes()),
            &mut data,
        )
        .map_err(|_| Error::invalid(i18n::backup_wrong_password()))?
        .len();
    data.truncate(len);
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::known_hosts::KnownHosts;
    use crate::model::Server;

    /// Cheap enough for debug builds.
    const TEST_COST: KdfCost = KdfCost {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };

    fn core() -> (Core, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("nexssh-backup-{}", util::random_id()));
        let core = Core::with_options(
            &dir,
            Secrets::in_memory(),
            Some(KnownHosts::new(dir.join("known_hosts"), None)),
        )
        .unwrap();
        (core, dir)
    }

    fn add(core: &Core, host: &str, password: Option<&str>) -> Server {
        let server = core
            .store
            .save_server(Server {
                host: host.into(),
                group: "Prod".into(),
                ..Server::default()
            })
            .unwrap();
        if let Some(p) = password {
            core.secrets
                .set(&Secrets::password_account(&server.id), p)
                .unwrap();
        }
        server
    }

    #[test]
    fn roundtrip_with_passwords() {
        let (from, from_dir) = core();
        let a = add(&from, "a.example", Some("secret-a"));
        add(&from, "b.example", None);
        from.secrets
            .set(&Secrets::jump_password_account(&a.id), "jump-a")
            .unwrap();
        std::fs::write(from_dir.join("known_hosts"), "a.example ssh-ed25519 AAAA\n").unwrap();
        let settings = serde_json::json!({ "theme": "graphite" });
        let created = create_with(&from, settings.clone(), Some("backup pw"), TEST_COST).unwrap();
        assert_eq!(created.passwords, 2);
        let text = String::from_utf8(created.json.clone()).unwrap();
        assert!(!text.contains("secret-a") && !text.contains("jump-a"));

        let (to, to_dir) = core();
        let old = add(&to, "old.example", Some("old"));
        std::fs::write(to_dir.join("known_hosts"), "old.example ssh-ed25519 BBBB").unwrap();

        let backup = parse(&created.json).unwrap();
        let summary = backup.summary();
        assert_eq!(
            (summary.servers, summary.groups, summary.passwords),
            (2, 1, true)
        );
        assert_eq!(backup.settings, settings);
        assert!(backup.open_passwords("wrong").is_err());
        let passwords = backup.open_passwords("backup pw").unwrap();
        let restored = restore(&to, &backup, &passwords).unwrap();
        assert_eq!((restored.servers, restored.passwords), (2, 2));

        let servers = to.store.snapshot();
        assert_eq!(servers, from.store.snapshot());
        assert!(to.store.get(&old.id).is_none());
        assert!(!to.secrets.has(&Secrets::password_account(&old.id)));
        assert_eq!(
            to.secrets
                .get(&Secrets::password_account(&a.id))
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("secret-a")
        );
        assert!(to.secrets.has(&Secrets::jump_password_account(&a.id)));
        assert_eq!(
            std::fs::read_to_string(to_dir.join("known_hosts")).unwrap(),
            "old.example ssh-ed25519 BBBB\na.example ssh-ed25519 AAAA\n"
        );
        // Again: nothing is added twice.
        restore(&to, &backup, &passwords).unwrap();
        assert_eq!(
            std::fs::read_to_string(to_dir.join("known_hosts"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn without_a_password_no_passwords_go_in() {
        let (from, _) = core();
        add(&from, "a.example", Some("secret-a"));
        let created = create_with(&from, Value::Null, None, TEST_COST).unwrap();
        assert_eq!(created.passwords, 0);
        let backup = parse(&created.json).unwrap();
        assert!(!backup.summary().passwords);
        assert!(
            !String::from_utf8(created.json)
                .unwrap()
                .contains("secret-a")
        );
    }

    #[test]
    fn passwords_only_reach_the_restored_servers() {
        let (from, _) = core();
        add(&from, "a.example", Some("secret-a"));
        let backup = parse(
            &create_with(&from, Value::Null, Some("pw"), TEST_COST)
                .unwrap()
                .json,
        )
        .unwrap();
        let mut passwords = backup.open_passwords("pw").unwrap();
        passwords.insert("agents-token".into(), "stolen".into());
        let (to, _) = core();
        let restored = restore(&to, &backup, &passwords).unwrap();
        assert_eq!(restored.passwords, 1);
        assert!(!to.secrets.has("agents-token"));
    }

    #[test]
    fn other_files_are_refused() {
        assert!(parse(b"not json").is_err());
        assert!(parse(br#"{"servers": []}"#).is_err());
        assert!(parse(br#"{"format": "nexssh-backup", "version": 99}"#).is_err());
        assert!(parse(br#"{"format": "nexssh-backup", "version": 1}"#).is_ok());
    }

    #[test]
    fn a_crafted_cost_is_refused() {
        let (from, _) = core();
        add(&from, "a.example", Some("secret-a"));
        let mut backup = parse(
            &create_with(&from, Value::Null, Some("pw"), TEST_COST)
                .unwrap()
                .json,
        )
        .unwrap();
        backup.passwords.as_mut().unwrap().memory_kib = u32::MAX;
        assert!(backup.open_passwords("pw").is_err());
    }
}
