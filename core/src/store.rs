//! Persistent server list stored as human-readable JSON (`servers.json`).
//!
//! The file contains no secrets: passwords and passphrases live in the OS keychain
//! (see [`crate::secrets`]). Writes are atomic (temp file + rename).

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::Server;
use crate::{i18n, util};

const FORMAT_VERSION: u32 = 1;

/// Everything stored in `servers.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StoreData {
    pub version: u32,
    /// Group order as shown in the sidebar (groups may be empty).
    pub groups: Vec<String>,
    pub servers: Vec<Server>,
}

/// Result of merging servers imported from `~/.ssh/config`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
}

pub struct ServerStore {
    path: PathBuf,
    data: RwLock<StoreData>,
}

impl ServerStore {
    /// Opens (or lazily creates) the store at `path`.
    ///
    /// A corrupted file is moved aside (`servers.json.broken-<time>`) instead of being
    /// overwritten, so user data is never silently lost.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut data = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<StoreData>(&bytes) {
                Ok(data) => data,
                Err(err) => {
                    let backup = path.with_extension(format!("json.broken-{}", util::now_secs()));
                    log::error!(
                        "cannot parse {}: {err}; moving it to {}",
                        path.display(),
                        backup.display()
                    );
                    std::fs::rename(&path, &backup)?;
                    StoreData::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => StoreData::default(),
            Err(err) => return Err(err.into()),
        };
        repair(&mut data);
        Ok(ServerStore {
            path,
            data: RwLock::new(data),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn snapshot(&self) -> StoreData {
        self.read().clone()
    }

    pub fn get(&self, id: &str) -> Option<Server> {
        self.read().servers.iter().find(|s| s.id == id).cloned()
    }

    /// Finds a server by id, ssh_config alias or name (case-insensitive).
    pub fn find(&self, key: &str) -> Option<Server> {
        let data = self.read();
        data.servers
            .iter()
            .find(|s| s.id == key)
            .or_else(|| {
                data.servers
                    .iter()
                    .find(|s| s.alias.as_deref() == Some(key))
            })
            .or_else(|| {
                data.servers
                    .iter()
                    .find(|s| s.name.eq_ignore_ascii_case(key))
            })
            .cloned()
    }

    /// Creates (empty id) or updates a server. Returns the stored version.
    pub fn save_server(&self, mut server: Server) -> Result<Server> {
        server.normalize()?;
        self.mutate(|data| {
            if server.id.is_empty() {
                server.id = unique_id(data);
                data.servers.push(server.clone());
            } else if let Some(existing) = data.servers.iter_mut().find(|s| s.id == server.id) {
                // The UI does not own this field; keep what we know.
                if server.last_used.is_none() {
                    server.last_used = existing.last_used;
                }
                *existing = server.clone();
            } else {
                data.servers.push(server.clone());
            }
            ensure_group(data, &server.group);
            Ok(server)
        })
    }

    pub fn delete_server(&self, id: &str) -> Result<Option<Server>> {
        self.mutate(|data| {
            let pos = data.servers.iter().position(|s| s.id == id);
            Ok(pos.map(|i| data.servers.remove(i)))
        })
    }

    /// Records a successful connection (used for "recent" ordering).
    pub fn touch(&self, id: &str) -> Result<()> {
        let now = util::now_secs();
        self.mutate(|data| {
            if let Some(s) = data.servers.iter_mut().find(|s| s.id == id) {
                s.last_used = Some(now);
            }
            Ok(())
        })
    }

    /// Replaces the group order. Groups still used by servers are kept even if omitted.
    pub fn set_groups(&self, groups: Vec<String>) -> Result<()> {
        self.mutate(|data| {
            let mut next: Vec<String> = Vec::new();
            for g in groups {
                let g = g.trim().to_string();
                if !g.is_empty() && !next.contains(&g) {
                    next.push(g);
                }
            }
            data.groups = next;
            repair(data);
            Ok(())
        })
    }

    pub fn rename_group(&self, from: &str, to: &str) -> Result<()> {
        let to = to.trim().to_string();
        if to.is_empty() {
            return Err(Error::invalid(i18n::group_name_empty()));
        }
        self.mutate(|data| {
            for s in data.servers.iter_mut().filter(|s| s.group == from) {
                s.group = to.clone();
            }
            if data.groups.contains(&to) {
                data.groups.retain(|g| g != from);
            } else if let Some(g) = data.groups.iter_mut().find(|g| *g == from) {
                *g = to.clone();
            } else {
                data.groups.push(to.clone());
            }
            Ok(())
        })
    }

    /// Deletes a group; its servers become ungrouped.
    pub fn remove_group(&self, name: &str) -> Result<()> {
        self.mutate(|data| {
            for s in data.servers.iter_mut().filter(|s| s.group == name) {
                s.group.clear();
            }
            data.groups.retain(|g| g != name);
            Ok(())
        })
    }

    /// Merges servers produced by the ssh_config importer.
    ///
    /// Entries are matched by alias first; a matching entry keeps its id, name, group
    /// and history, and only connection fields are refreshed. Entries that duplicate an
    /// existing server (same name, or same user, host, port, key and jump host) are skipped.
    pub fn merge_imported(&self, imported: Vec<Server>) -> Result<ImportSummary> {
        self.mutate(|data| {
            let mut summary = ImportSummary::default();
            for mut incoming in imported {
                if incoming.normalize().is_err() {
                    continue;
                }
                let by_alias = incoming.alias.as_ref().and_then(|alias| {
                    data.servers
                        .iter()
                        .position(|s| s.alias.as_ref() == Some(alias))
                });
                if let Some(i) = by_alias {
                    let existing = &mut data.servers[i];
                    let mut updated = existing.clone();
                    updated.host = incoming.host;
                    updated.port = incoming.port;
                    updated.user = incoming.user;
                    updated.identity_file = incoming.identity_file;
                    updated.jump_host = incoming.jump_host;
                    updated.keepalive_secs = incoming.keepalive_secs;
                    updated.connect_timeout_secs = incoming.connect_timeout_secs;
                    updated.forwards = incoming.forwards;
                    if updated.auth != crate::model::AuthKind::Password {
                        updated.auth = incoming.auth;
                    }
                    if *existing == updated {
                        summary.unchanged += 1;
                    } else {
                        *existing = updated;
                        summary.updated += 1;
                    }
                    continue;
                }
                // Same name, or a manually added server connecting the very same way.
                // (Imported entries are distinct aliases even for one address: a jump
                // host may be referenced by its alias.)
                let duplicate = data.servers.iter().any(|s| {
                    incoming
                        .alias
                        .as_deref()
                        .is_some_and(|a| s.name.eq_ignore_ascii_case(a))
                        || (s.alias.is_none()
                            && s.host.eq_ignore_ascii_case(&incoming.host)
                            && s.port == incoming.port
                            && s.user == incoming.user
                            && s.identity_file == incoming.identity_file
                            && s.jump_host == incoming.jump_host)
                });
                if duplicate {
                    summary.unchanged += 1;
                    continue;
                }
                incoming.id = unique_id(data);
                ensure_group(data, &incoming.group);
                data.servers.push(incoming);
                summary.added += 1;
            }
            Ok(summary)
        })
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, StoreData> {
        self.data.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Applies `f` to a copy of the data, persists it, and only then commits it in memory.
    fn mutate<T>(&self, f: impl FnOnce(&mut StoreData) -> Result<T>) -> Result<T> {
        let mut guard = self.data.write().unwrap_or_else(|e| e.into_inner());
        let mut next = guard.clone();
        let out = f(&mut next)?;
        if next != *guard {
            next.version = FORMAT_VERSION;
            let json = serde_json::to_vec_pretty(&next)?;
            util::atomic_write(&self.path, &json)?;
            *guard = next;
        }
        Ok(out)
    }
}

fn unique_id(data: &StoreData) -> String {
    loop {
        let id = util::random_id();
        if !data.servers.iter().any(|s| s.id == id) {
            return id;
        }
    }
}

fn ensure_group(data: &mut StoreData, group: &str) {
    if !group.is_empty() && !data.groups.iter().any(|g| g == group) {
        data.groups.push(group.to_string());
    }
}

/// Makes loaded data consistent: unique non-empty ids, every used group listed.
fn repair(data: &mut StoreData) {
    let mut seen = std::collections::HashSet::new();
    for i in 0..data.servers.len() {
        let id = data.servers[i].id.clone();
        if id.is_empty() || !seen.insert(id) {
            let fresh = unique_id(data);
            seen.insert(fresh.clone());
            data.servers[i].id = fresh;
        }
    }
    let used: Vec<String> = data.servers.iter().map(|s| s.group.clone()).collect();
    for g in used {
        ensure_group(data, &g);
    }
    data.groups.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AuthKind;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexssh-test-{}", util::random_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn server(host: &str, group: &str) -> Server {
        Server {
            host: host.into(),
            group: group.into(),
            ..Server::default()
        }
    }

    #[test]
    fn crud_roundtrip() {
        let path = temp_path("servers.json");
        let store = ServerStore::open(&path).unwrap();
        let a = store
            .save_server(server("a.example", "Production"))
            .unwrap();
        assert!(!a.id.is_empty());
        assert_eq!(a.name, "a.example");
        let b = store
            .save_server(server("b.example", "Development"))
            .unwrap();

        let reopened = ServerStore::open(&path).unwrap();
        let data = reopened.snapshot();
        assert_eq!(data.servers.len(), 2);
        assert_eq!(data.groups, vec!["Production", "Development"]);
        assert_eq!(reopened.find("B.EXAMPLE").unwrap().id, b.id);

        let mut edited = a.clone();
        edited.port = 2222;
        reopened.save_server(edited).unwrap();
        assert_eq!(reopened.get(&a.id).unwrap().port, 2222);

        reopened.delete_server(&b.id).unwrap();
        assert!(reopened.get(&b.id).is_none());
    }

    #[test]
    fn invalid_server_is_rejected_without_writing() {
        let path = temp_path("servers.json");
        let store = ServerStore::open(&path).unwrap();
        assert!(store.save_server(server("", "")).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn groups_rename_and_remove() {
        let path = temp_path("servers.json");
        let store = ServerStore::open(&path).unwrap();
        let s = store.save_server(server("h", "Old")).unwrap();
        store.rename_group("Old", "New").unwrap();
        assert_eq!(store.get(&s.id).unwrap().group, "New");
        assert_eq!(store.snapshot().groups, vec!["New"]);
        store.remove_group("New").unwrap();
        assert_eq!(store.get(&s.id).unwrap().group, "");
        assert!(store.snapshot().groups.is_empty());
    }

    #[test]
    fn corrupted_file_is_preserved() {
        let path = temp_path("servers.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let store = ServerStore::open(&path).unwrap();
        assert!(store.snapshot().servers.is_empty());
        let dir = path.parent().unwrap();
        let backups = std::fs::read_dir(dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("broken")
            })
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn import_keeps_distinct_aliases_for_the_same_address() {
        let path = temp_path("servers.json");
        let store = ServerStore::open(&path).unwrap();
        store
            .save_server(Server {
                name: "manual".into(),
                host: "10.0.0.1".into(),
                user: "root".into(),
                ..Server::default()
            })
            .unwrap();
        let imported = |alias: &str, key: Option<&str>| Server {
            alias: Some(alias.into()),
            name: alias.into(),
            host: "10.0.0.1".into(),
            user: "root".into(),
            identity_file: key.map(Into::into),
            ..Server::default()
        };
        let summary = store
            .merge_imported(vec![
                imported("same-as-manual", None),
                imported("with-key", Some("~/.ssh/id_rsa")),
                imported("bastion", Some("~/.ssh/id_rsa")),
                imported("MANUAL", Some("~/.ssh/other")),
            ])
            .unwrap();
        assert_eq!(summary.added, 2, "{summary:?}");
        assert_eq!(summary.unchanged, 2);
        assert!(store.find("with-key").is_some());
        assert!(store.find("bastion").is_some());
    }

    #[test]
    fn import_merges_by_alias() {
        let path = temp_path("servers.json");
        let store = ServerStore::open(&path).unwrap();
        let imported = vec![Server {
            alias: Some("web".into()),
            host: "10.0.0.1".into(),
            group: "SSH config".into(),
            ..Server::default()
        }];
        let first = store.merge_imported(imported.clone()).unwrap();
        assert_eq!(first.added, 1);
        let again = store.merge_imported(imported).unwrap();
        assert_eq!(again.unchanged, 1);

        // User moves it to another group; a re-import with a new address keeps the group.
        let mut s = store.find("web").unwrap();
        s.group = "Production".into();
        s.auth = AuthKind::Password;
        store.save_server(s.clone()).unwrap();
        let changed = store
            .merge_imported(vec![Server {
                alias: Some("web".into()),
                host: "10.0.0.2".into(),
                group: "SSH config".into(),
                ..Server::default()
            }])
            .unwrap();
        assert_eq!(changed.updated, 1);
        let s2 = store.get(&s.id).unwrap();
        assert_eq!(s2.host, "10.0.0.2");
        assert_eq!(s2.group, "Production");
        assert_eq!(s2.auth, AuthKind::Password);
    }
}
