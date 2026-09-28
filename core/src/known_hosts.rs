//! Host key verification compatible with OpenSSH's `known_hosts` format.
//!
//! Two files are consulted:
//! * NexSSH's own `known_hosts` (in the app data directory) — decisions made in NexSSH
//!   are written here, so the user's `~/.ssh` is never modified;
//! * the user's `~/.ssh/known_hosts` (read-only), so hosts trusted in OpenSSH are trusted here.
//!
//! Supports plain and hashed (`|1|salt|hash`) host names, `[host]:port` entries,
//! wildcards, negations and `@revoked` markers. Unparseable lines are skipped instead of
//! failing the whole check.

use std::path::{Path, PathBuf};

use data_encoding::BASE64;
use hmac::{Hmac, KeyInit, Mac};
use russh::keys::{Algorithm, HashAlg, PublicKey};
use serde::Serialize;
use sha1::Sha1;

use crate::error::Result;
use crate::util;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum HostKeyStatus {
    /// A matching key is on record.
    Trusted,
    /// No key of this type is on record for the host.
    Unknown,
    /// A different key of the same type is on record: possible man-in-the-middle attack.
    #[serde(rename_all = "camelCase")]
    Changed {
        file: String,
        line: usize,
        known_fingerprint: String,
    },
    /// The key is explicitly marked `@revoked`.
    Revoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    CertAuthority,
    Revoked,
}

struct Entry {
    marker: Option<Marker>,
    line: usize,
    key: PublicKey,
}

pub struct KnownHosts {
    app_file: PathBuf,
    user_file: Option<PathBuf>,
}

impl KnownHosts {
    pub fn new(app_file: PathBuf, user_file: Option<PathBuf>) -> Self {
        KnownHosts {
            app_file,
            user_file,
        }
    }

    /// NexSSH's file in `app_dir` plus `~/.ssh/known_hosts`.
    pub fn with_defaults(app_dir: &Path) -> Self {
        KnownHosts::new(
            app_dir.join("known_hosts"),
            util::ssh_dir().map(|d| d.join("known_hosts")),
        )
    }

    fn files(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.app_file.as_path()).chain(self.user_file.as_deref())
    }

    pub fn check(&self, host: &str, port: u16, key: &PublicKey) -> HostKeyStatus {
        let name = lookup_name(host, port);
        let per_file: Vec<(PathBuf, Vec<Entry>)> = self
            .files()
            .map(|f| (f.to_path_buf(), entries_for(f, &name)))
            .collect();

        if per_file
            .iter()
            .flat_map(|(_, e)| e)
            .any(|e| e.marker == Some(Marker::Revoked) && same_key(&e.key, key))
        {
            return HostKeyStatus::Revoked;
        }
        // NexSSH's own decisions win over ~/.ssh/known_hosts (e.g. an accepted key change).
        for (file, entries) in &per_file {
            let mut mismatch = None;
            for e in entries.iter().filter(|e| e.marker.is_none()) {
                if same_key(&e.key, key) {
                    return HostKeyStatus::Trusted;
                }
                if same_family(&e.key.algorithm(), &key.algorithm()) && mismatch.is_none() {
                    mismatch = Some(e);
                }
            }
            if let Some(e) = mismatch {
                return HostKeyStatus::Changed {
                    file: util::contract_tilde(file),
                    line: e.line,
                    known_fingerprint: e.key.fingerprint(HashAlg::Sha256).to_string(),
                };
            }
        }
        HostKeyStatus::Unknown
    }

    /// Key algorithms on record for the host; used to prefer them during key exchange,
    /// like OpenSSH does, so a host known by its RSA key is not reported as unknown.
    pub fn known_algorithms(&self, host: &str, port: u16) -> Vec<Algorithm> {
        let name = lookup_name(host, port);
        let mut algs: Vec<Algorithm> = Vec::new();
        for f in self.files() {
            for e in entries_for(f, &name) {
                if e.marker.is_none() && !algs.iter().any(|a| same_family(a, &e.key.algorithm())) {
                    algs.push(e.key.algorithm());
                }
            }
        }
        algs
    }

    /// Records `key` for the host in NexSSH's file, replacing older keys of the same type.
    pub fn learn(&self, host: &str, port: u16, key: &PublicKey) -> Result<()> {
        let name = lookup_name(host, port);
        let existing = std::fs::read_to_string(&self.app_file).unwrap_or_default();
        let mut out = String::with_capacity(existing.len() + 128);
        for line in existing.lines() {
            let replace = parse_line(line).is_some_and(|(marker, hosts, parsed)| {
                marker.is_none()
                    && hosts_match(hosts, &name)
                    && same_family(&parsed.algorithm(), &key.algorithm())
            });
            if !replace {
                out.push_str(line);
                out.push('\n');
            }
        }
        let mut plain = key.clone();
        plain.set_comment("");
        out.push_str(&name);
        out.push(' ');
        out.push_str(plain.to_openssh().map_err(russh::keys::Error::from)?.trim());
        out.push('\n');
        util::atomic_write(&self.app_file, out.as_bytes())?;
        Ok(())
    }
}

/// How OpenSSH names a host in known_hosts: `host` on port 22, else `[host]:port`.
fn lookup_name(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

fn entries_for(file: &Path, name: &str) -> Vec<Entry> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let (marker, hosts, key) = parse_line(line)?;
            (marker != Some(Marker::CertAuthority) && hosts_match(hosts, name)).then_some(Entry {
                marker,
                line: i + 1,
                key,
            })
        })
        .collect()
}

fn parse_line(line: &str) -> Option<(Option<Marker>, &str, PublicKey)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut fields = line.split_whitespace();
    let mut hosts = fields.next()?;
    let marker = match hosts {
        "@revoked" => Some(Marker::Revoked),
        "@cert-authority" => Some(Marker::CertAuthority),
        m if m.starts_with('@') => return None,
        _ => None,
    };
    if marker.is_some() {
        hosts = fields.next()?;
    }
    let key_type = fields.next()?;
    let key_data = fields.next()?;
    let key = PublicKey::from_openssh(&format!("{key_type} {key_data}")).ok()?;
    Some((marker, hosts, key))
}

fn hosts_match(hosts: &str, name: &str) -> bool {
    if hosts.starts_with("|1|") {
        return hashed_match(hosts, name);
    }
    util::pattern_list_match(hosts.split(','), name)
}

fn hashed_match(entry: &str, name: &str) -> bool {
    let mut parts = entry.split('|').skip(2);
    let (Some(salt), Some(hash)) = (parts.next(), parts.next()) else {
        return false;
    };
    let (Ok(salt), Ok(hash)) = (
        BASE64.decode(salt.as_bytes()),
        BASE64.decode(hash.as_bytes()),
    ) else {
        return false;
    };
    let Ok(mac) = Hmac::<Sha1>::new_from_slice(&salt) else {
        return false;
    };
    mac.chain_update(name.as_bytes())
        .verify_slice(&hash)
        .is_ok()
}

fn same_key(a: &PublicKey, b: &PublicKey) -> bool {
    a.key_data() == b.key_data()
}

/// RSA keys match regardless of the signature hash (`ssh-rsa`, `rsa-sha2-256/512`).
fn same_family(a: &Algorithm, b: &Algorithm) -> bool {
    matches!((a, b), (Algorithm::Rsa { .. }, Algorithm::Rsa { .. })) || a == b
}

/// Default host key algorithm order with the ones already on record moved to the front.
pub fn preferred_host_key_algorithms(known: &[Algorithm]) -> Vec<Algorithm> {
    let defaults = russh::Preferred::DEFAULT.key.to_vec();
    if known.is_empty() {
        return defaults;
    }
    let (mut first, rest): (Vec<Algorithm>, Vec<Algorithm>) = defaults
        .into_iter()
        .partition(|alg| known.iter().any(|k| same_family(k, alg)));
    first.extend(rest);
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED_A: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const ED_B: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";
    const RSA: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAgQCyxDVKq99ebCNbyYJNYPqEg4c6QFxxufBu4GY3CSaQShchgp9y6DDk779WH26XukJOFJFpzKOoP4gZarDGStcy++bFWhr7wKIlPzNOnex6Rn9d6nabtYowmoLFhLt7tA4hIXRLc0tWeIm0LP65MW355+TsxbVuzLFbNfhWWtQHqw==";

    fn key(s: &str) -> PublicKey {
        PublicKey::from_openssh(s).unwrap()
    }

    fn setup(user_lines: &str) -> (KnownHosts, PathBuf) {
        let dir = std::env::temp_dir().join(format!("nexssh-kh-{}", util::random_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let user = dir.join("user_known_hosts");
        std::fs::write(&user, user_lines).unwrap();
        (KnownHosts::new(dir.join("known_hosts"), Some(user)), dir)
    }

    #[test]
    fn plain_hashed_and_port_entries() {
        let (kh, _dir) = setup(&format!(
            "# comment\n\
             garbage line that is not a key\n\
             [localhost]:2222 {ED_A}\n\
             |1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak= {ED_B}\n\
             *.corp,!secret.corp {ED_A}\n"
        ));
        assert_eq!(
            kh.check("localhost", 2222, &key(ED_A)),
            HostKeyStatus::Trusted
        );
        assert_eq!(
            kh.check("localhost", 22, &key(ED_A)),
            HostKeyStatus::Unknown
        );
        assert_eq!(
            kh.check("example.com", 22, &key(ED_B)),
            HostKeyStatus::Trusted
        );
        assert_eq!(kh.check("web.corp", 22, &key(ED_A)), HostKeyStatus::Trusted);
        assert_eq!(
            kh.check("secret.corp", 22, &key(ED_A)),
            HostKeyStatus::Unknown
        );
        assert!(matches!(
            kh.check("localhost", 2222, &key(ED_B)),
            HostKeyStatus::Changed { line: 3, .. }
        ));
    }

    #[test]
    fn learn_overrides_changed_key_without_touching_user_file() {
        let (kh, _dir) = setup(&format!("example.org {ED_A}\n"));
        let user_before = std::fs::read_to_string(kh.user_file.as_ref().unwrap()).unwrap();
        assert!(matches!(
            kh.check("example.org", 22, &key(ED_B)),
            HostKeyStatus::Changed { .. }
        ));
        kh.learn("example.org", 22, &key(ED_B)).unwrap();
        assert_eq!(
            kh.check("example.org", 22, &key(ED_B)),
            HostKeyStatus::Trusted
        );
        // Learning again replaces instead of accumulating lines.
        kh.learn("example.org", 22, &key(ED_B)).unwrap();
        let app = std::fs::read_to_string(&kh.app_file).unwrap();
        assert_eq!(app.lines().count(), 1);
        assert!(app.starts_with("example.org ssh-ed25519 "));
        let user_after = std::fs::read_to_string(kh.user_file.as_ref().unwrap()).unwrap();
        assert_eq!(user_before, user_after);
    }

    #[test]
    fn revoked_keys_are_rejected() {
        let (kh, _dir) = setup(&format!("@revoked * {ED_A}\nhost {ED_A}\n"));
        assert_eq!(kh.check("host", 22, &key(ED_A)), HostKeyStatus::Revoked);
    }

    #[test]
    fn known_algorithms_reorder_preferences() {
        let (kh, _dir) = setup(&format!("rsa-only {RSA}\n"));
        let known = kh.known_algorithms("rsa-only", 22);
        assert_eq!(known.len(), 1);
        let prefs = preferred_host_key_algorithms(&known);
        assert!(matches!(prefs[0], Algorithm::Rsa { .. }));
        assert_eq!(prefs.len(), russh::Preferred::DEFAULT.key.len());
    }
}
