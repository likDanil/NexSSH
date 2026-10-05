//! Remote files edited in a program of this computer.
//!
//! A file is copied into a folder of its own under the temporary folder (so the program
//! shows its real name and highlights it by it), and the program opens it there. Every time
//! it is saved, it goes back to the server on its session's connection. The copy is polled
//! rather than watched: editors save in place, or through a temporary file and a rename, and
//! a path that is looked at again catches them all. A save goes up once the file stopped
//! changing for a moment.
//!
//! Before writing, the server's copy is compared with the one downloaded (size and time of
//! change): when someone changed it in between, nothing is overwritten until the user
//! decides. Saves made while the session is disconnected go up when it is connected again
//! (a session keeps its id across reconnects).
//!
//! A file the user may not read or write over SFTP can be read and written with `sudo`
//! (`sudo cat`, `sudo tee`, on a channel without a terminal, see [`crate::exec`]). A password
//! typed for it is kept in memory for the session, like the secrets typed while connecting,
//! and always consumed by sudo itself: with `-k` sudo asks for it even when it remembers
//! the user, so it can never end up in the file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

use crate::error::Error;
use crate::exec::{ExecOptions, ExecOutput, Exit};
use crate::session::{SessionId, SessionManager};
use crate::sftp::{self, EntryKind};
use crate::{i18n, util};

pub type EditId = u64;

/// Larger files are not edited this way: an editor is not the tool for them.
pub const MAX_SIZE: u64 = 100 * 1024 * 1024;
/// How often the copy is looked at; a save goes up once it is unchanged across one look.
const POLL: Duration = Duration::from_millis(400);
const SUDO_TIMEOUT: Duration = Duration::from_secs(120);
/// A save waiting for its session is tried again every this many looks (about 5 s).
const WAITING_RETRY: u64 = 12;
/// Folders of earlier runs are removed at start-up once they are this old.
const KEEP_OLD: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EditState {
    /// The server has what was saved last.
    Synced,
    /// A save is on its way to the server.
    Uploading,
    /// The server's copy changed since it was downloaded: the user decides.
    Conflict,
    /// The server refused writing over SFTP, or sudo wants a password: the user decides.
    Denied,
    /// The session is not connected: the save goes up when it is.
    Waiting,
    /// The last save did not go up (`error` says why); the next one tries again.
    Failed,
}

/// A file being edited, as the page shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditInfo {
    pub id: EditId,
    pub session_id: SessionId,
    pub remote_path: String,
    pub local_path: String,
    pub name: String,
    pub state: EditState,
    /// Unix time in milliseconds of the last save that reached the server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<u64>,
    /// Read and written with sudo.
    pub sudo: bool,
    /// `Denied` because sudo wants a password (none known, or the one given was refused).
    pub needs_password: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Why a file could not be opened.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Refusal {
    /// The server refused reading it over SFTP: sudo may.
    Denied {
        message: String,
    },
    /// sudo wants a password; `wrong`: the one given was refused (`message` says how).
    Password {
        wrong: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Failed {
        message: String,
    },
}

impl From<Error> for Refusal {
    fn from(e: Error) -> Self {
        match e {
            Error::Denied(message) => Refusal::Denied { message },
            other => Refusal::Failed {
                message: other.to_string(),
            },
        }
    }
}

/// What to do about a file that changed on the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Resolution {
    /// Write the local copy over the server's.
    Overwrite,
    /// Replace the local copy with the server's (the editor reloads it).
    Reload,
}

/// A remote file's size and time of change, to notice that someone else changed it.
type Stamp = (u64, Option<u64>);

/// A local file's size and time of change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LocalStamp {
    len: u64,
    modified: Option<SystemTime>,
}

fn local_stamp(path: &Path) -> Option<LocalStamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(LocalStamp {
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

struct Item {
    info: EditInfo,
    /// The server's copy as downloaded or last written; `None` when it could not be looked at.
    remote: Option<Stamp>,
    /// The local copy as downloaded or last sent.
    local: Option<LocalStamp>,
    /// The local copy whose upload failed: tried again only once it changes.
    failed: Option<LocalStamp>,
    /// An upload is running.
    busy: bool,
    task: Option<JoinHandle<()>>,
}

/// How sudo is run: without a password, or reading one from the first line of the input.
#[derive(Clone)]
enum SudoMode {
    NoPassword,
    Password(Zeroizing<String>),
}

impl SudoMode {
    /// `sudo …` and what goes before the command's own input.
    fn prefix(&self) -> (&'static str, Vec<u8>) {
        match self {
            SudoMode::NoPassword => ("sudo -n", Vec::new()),
            SudoMode::Password(password) => {
                let mut input = password.as_bytes().to_vec();
                input.push(b'\n');
                ("sudo -k -S -p ''", input)
            }
        }
    }
}

/// Why a save did not go up.
enum Problem {
    Conflict,
    Denied(String),
    Password {
        wrong: bool,
        message: Option<String>,
    },
    Disconnected,
    Failed(String),
}

impl From<Error> for Problem {
    fn from(e: Error) -> Self {
        match e {
            Error::Denied(message) => Problem::Denied(message),
            Error::Disconnected(_) => Problem::Disconnected,
            other => Problem::Failed(other.to_string()),
        }
    }
}

impl From<Refusal> for Problem {
    fn from(r: Refusal) -> Self {
        match r {
            Refusal::Denied { message } => Problem::Denied(message),
            Refusal::Password { wrong, message } => Problem::Password { wrong, message },
            Refusal::Failed { message } => Problem::Failed(message),
        }
    }
}

type Notify = Box<dyn Fn(Vec<EditInfo>) + Send + Sync>;

/// Every file being edited.
pub struct Edits {
    sessions: SessionManager,
    root: PathBuf,
    items: Mutex<HashMap<EditId, Item>>,
    /// sudo passwords typed for sessions.
    passwords: Mutex<HashMap<SessionId, Zeroizing<String>>>,
    next: AtomicU64,
    notify: Notify,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `'text'` for a POSIX shell.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The last line sudo or a command wrote on standard error.
fn last_error_line(output: &ExecOutput) -> Option<String> {
    let (head, tail) = output.stderr.parts();
    let mut text = String::from_utf8_lossy(head).into_owned();
    text.push_str(&String::from_utf8_lossy(&tail));
    text.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(str::to_string)
}

fn succeeded(output: &ExecOutput) -> bool {
    !output.timed_out && output.exit == Some(Exit::Code(0))
}

impl Edits {
    /// Copies of edited files go into folders under `root`; `notify` gets the list on every
    /// change. Folders that earlier runs left behind are removed once they are old.
    pub fn new(sessions: SessionManager, root: PathBuf, notify: Notify) -> Arc<Edits> {
        let old = root.clone();
        std::thread::spawn(move || remove_old(&old, KEEP_OLD));
        Arc::new(Edits {
            sessions,
            root,
            items: Mutex::new(HashMap::new()),
            passwords: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            notify,
        })
    }

    pub fn list(&self) -> Vec<EditInfo> {
        let mut list: Vec<EditInfo> = lock(&self.items).values().map(|i| i.info.clone()).collect();
        list.sort_by_key(|i| i.id);
        list
    }

    pub fn get(&self, id: EditId) -> Option<EditInfo> {
        lock(&self.items).get(&id).map(|i| i.info.clone())
    }

    fn changed(&self) {
        (self.notify)(self.list());
    }

    /// Opens `path` of the session over SFTP: its copy, ready for an editor. A file edited
    /// already keeps its copy.
    pub async fn open(
        self: &Arc<Self>,
        session: SessionId,
        path: &str,
    ) -> Result<EditInfo, Refusal> {
        self.open_with(session, path, None).await
    }

    /// Opens `path` with sudo: with `password`, else one typed earlier for the session, else
    /// sudo without a password.
    pub async fn open_sudo(
        self: &Arc<Self>,
        session: SessionId,
        path: &str,
        password: Option<String>,
    ) -> Result<EditInfo, Refusal> {
        let mode = self.sudo_mode(session, password).await?;
        self.open_with(session, path, Some(mode)).await
    }

    async fn open_with(
        self: &Arc<Self>,
        session: SessionId,
        path: &str,
        sudo: Option<SudoMode>,
    ) -> Result<EditInfo, Refusal> {
        if let Some(info) = self.find(session, path) {
            return Ok(info);
        }
        let sftp = self.sessions.sftp(session).await?;
        // With sudo the file may be out of reach of SFTP; it is read anyway.
        let meta = match sftp.stat(path).await {
            Ok(meta) => Some(meta),
            Err(Error::Denied(_)) if sudo.is_some() => None,
            Err(e) => return Err(e.into()),
        };
        if let Some(meta) = &meta {
            if meta.kind == EntryKind::Dir {
                return Err(Error::invalid(i18n::sftp_is_folder(path)).into());
            }
            if meta.size > MAX_SIZE {
                return Err(Error::invalid(i18n::edit_too_large(MAX_SIZE / 1024 / 1024)).into());
            }
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let dir = self.root.join(util::random_id());
        std::fs::create_dir_all(&dir).map_err(|e| Refusal::Failed {
            message: i18n::local_write_failed(&dir.display().to_string(), e),
        })?;
        let local = dir.join(sftp::safe_name(&sftp::base_name(path)));
        if let Err(refusal) = self.fetch(session, path, &local, sudo.as_ref()).await {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(refusal);
        }
        let info = EditInfo {
            id,
            session_id: session,
            remote_path: path.to_string(),
            local_path: local.display().to_string(),
            name: sftp::base_name(path),
            state: EditState::Synced,
            saved_at: None,
            sudo: sudo.is_some(),
            needs_password: false,
            error: None,
        };
        let task = tokio::spawn(watch(Arc::downgrade(self), id));
        lock(&self.items).insert(
            id,
            Item {
                info: info.clone(),
                remote: meta.map(|m| (m.size, m.modified)),
                local: local_stamp(&local),
                failed: None,
                busy: false,
                task: Some(task),
            },
        );
        self.changed();
        Ok(info)
    }

    fn find(&self, session: SessionId, path: &str) -> Option<EditInfo> {
        lock(&self.items)
            .values()
            .find(|i| i.info.session_id == session && i.info.remote_path == path)
            .map(|i| i.info.clone())
    }

    /// Copies the server's file into `local` (replacing it).
    async fn fetch(
        &self,
        session: SessionId,
        remote: &str,
        local: &Path,
        sudo: Option<&SudoMode>,
    ) -> Result<(), Refusal> {
        let write = |data: &[u8]| {
            std::fs::write(local, data).map_err(|e| Refusal::Failed {
                message: i18n::local_write_failed(&local.display().to_string(), e),
            })
        };
        if let Some(mode) = sudo {
            let data = self.sudo_read(session, remote, mode).await?;
            return write(&data);
        }
        // Downloads never overwrite: into a folder of its own next to the copy, then over it.
        let parent = local.parent().unwrap_or(Path::new("."));
        let staging = parent.join(format!(".{}", util::random_id()));
        let sftp = self.sessions.sftp(session).await?;
        let cancel = AtomicBool::new(false);
        let result = sftp.download(remote, &staging, &mut |_| {}, &cancel).await;
        let moved = result.map_err(Refusal::from).and_then(|got| {
            std::fs::rename(&got, local).map_err(|e| Refusal::Failed {
                message: i18n::local_write_failed(&local.display().to_string(), e),
            })
        });
        let _ = std::fs::remove_dir_all(&staging);
        moved
    }

    /// Sends the local copy to the server; `force` writes over a server copy that changed.
    async fn upload(self: &Arc<Self>, id: EditId, force: bool) {
        let snapshot = {
            let mut items = lock(&self.items);
            let Some(item) = items.get_mut(&id) else {
                return;
            };
            if item.busy {
                return;
            }
            item.busy = true;
            item.info.state = EditState::Uploading;
            item.info.error = None;
            (
                item.info.session_id,
                item.info.remote_path.clone(),
                PathBuf::from(&item.info.local_path),
                item.remote,
                item.info.sudo,
            )
        };
        self.changed();
        let (session, remote, local, remote_stamp, sudo) = snapshot;
        let result = self
            .send(session, &remote, &local, remote_stamp, sudo, force)
            .await;
        {
            let mut items = lock(&self.items);
            let Some(item) = items.get_mut(&id) else {
                return;
            };
            item.busy = false;
            item.info.needs_password = false;
            match result {
                Ok((stamp, sent)) => {
                    item.info.state = EditState::Synced;
                    item.info.saved_at = Some(now_ms());
                    item.remote = stamp;
                    item.local = Some(sent);
                    item.failed = None;
                }
                Err(Problem::Conflict) => item.info.state = EditState::Conflict,
                Err(Problem::Denied(message)) => {
                    item.info.state = EditState::Denied;
                    item.info.error = Some(message);
                }
                Err(Problem::Password { wrong, message }) => {
                    item.info.state = EditState::Denied;
                    item.info.needs_password = true;
                    item.info.error = message.filter(|_| wrong);
                }
                Err(Problem::Disconnected) => item.info.state = EditState::Waiting,
                Err(Problem::Failed(message)) => {
                    item.info.state = EditState::Failed;
                    item.info.error = Some(message);
                    item.failed = local_stamp(&local);
                }
            }
        }
        self.changed();
    }

    async fn send(
        &self,
        session: SessionId,
        remote: &str,
        local: &Path,
        remote_stamp: Option<Stamp>,
        sudo: bool,
        force: bool,
    ) -> Result<(Option<Stamp>, LocalStamp), Problem> {
        // Taken before reading: a save made meanwhile goes up next time.
        let sent = local_stamp(local)
            .ok_or_else(|| Problem::Failed(i18n::edit_copy_gone(&local.display().to_string())))?;
        let data = tokio::fs::read(local).await.map_err(|e| {
            Problem::Failed(i18n::local_read_failed(&local.display().to_string(), e))
        })?;
        let sftp = self.sessions.sftp(session).await?;
        if !force {
            let now = sftp.stat(remote).await.ok().map(|m| (m.size, m.modified));
            if now != remote_stamp {
                return Err(Problem::Conflict);
            }
        }
        if sudo {
            let mode = self.sudo_mode(session, None).await?;
            self.sudo_write(session, remote, &data, &mode).await?;
        } else {
            sftp.write_whole(remote, &data).await?;
        }
        let stamp = sftp.stat(remote).await.ok().map(|m| (m.size, m.modified));
        Ok((stamp, sent))
    }

    /// Settles a conflict.
    pub async fn resolve(self: &Arc<Self>, id: EditId, how: Resolution) -> Option<EditInfo> {
        match how {
            Resolution::Overwrite => self.upload(id, true).await,
            Resolution::Reload => self.reload(id).await,
        }
        self.get(id)
    }

    async fn reload(self: &Arc<Self>, id: EditId) {
        let Some(info) = self.get(id) else { return };
        let local = PathBuf::from(&info.local_path);
        let result = async {
            let mode = match info.sudo {
                true => Some(self.sudo_mode(info.session_id, None).await?),
                false => None,
            };
            self.fetch(info.session_id, &info.remote_path, &local, mode.as_ref())
                .await?;
            let sftp = self.sessions.sftp(info.session_id).await?;
            let stamp = sftp
                .stat(&info.remote_path)
                .await
                .ok()
                .map(|m| (m.size, m.modified));
            Ok::<_, Refusal>(stamp)
        }
        .await;
        {
            let mut items = lock(&self.items);
            let Some(item) = items.get_mut(&id) else {
                return;
            };
            match result {
                Ok(stamp) => {
                    item.remote = stamp;
                    item.local = local_stamp(&local);
                    item.failed = None;
                    item.info.state = EditState::Synced;
                    item.info.error = None;
                }
                Err(refusal) => {
                    item.info.state = EditState::Failed;
                    item.info.error = Some(match refusal {
                        Refusal::Denied { message } | Refusal::Failed { message } => message,
                        Refusal::Password { .. } => i18n::sudo_wrong_password(),
                    });
                }
            }
        }
        self.changed();
    }

    /// Writes the file with sudo from now on (`password`: one for sudo), and sends it.
    pub async fn use_sudo(
        self: &Arc<Self>,
        id: EditId,
        password: Option<String>,
    ) -> Option<EditInfo> {
        let session = self.get(id)?.session_id;
        if let Some(password) = password {
            match self.sudo_mode(session, Some(password)).await {
                Ok(_) => {}
                Err(refusal) => {
                    if let Some(item) = lock(&self.items).get_mut(&id) {
                        let (wrong, message) = match refusal {
                            Refusal::Password { wrong, message } => (wrong, message),
                            Refusal::Denied { message } | Refusal::Failed { message } => {
                                (true, Some(message))
                            }
                        };
                        item.info.state = EditState::Denied;
                        item.info.needs_password = true;
                        item.info.error = message.filter(|_| wrong);
                    }
                    self.changed();
                    return self.get(id);
                }
            }
        }
        if let Some(item) = lock(&self.items).get_mut(&id) {
            item.info.sudo = true;
        }
        self.upload(id, false).await;
        self.get(id)
    }

    /// Stops editing: the file is no longer sent, and its copy is removed.
    pub fn stop(&self, id: EditId) {
        let item = lock(&self.items).remove(&id);
        if let Some(item) = item {
            forget(item, false);
            self.changed();
        }
    }

    /// Stops editing the files of a session (its tab was closed). Copies holding saves that
    /// did not reach the server stay.
    pub fn stop_session(&self, session: SessionId) {
        let gone: Vec<Item> = {
            let mut items = lock(&self.items);
            let ids: Vec<EditId> = items
                .iter()
                .filter(|(_, i)| i.info.session_id == session)
                .map(|(id, _)| *id)
                .collect();
            ids.into_iter().filter_map(|id| items.remove(&id)).collect()
        };
        lock(&self.passwords).remove(&session);
        if !gone.is_empty() {
            gone.into_iter().for_each(|item| forget(item, true));
            self.changed();
        }
    }

    pub fn stop_all(&self) {
        let gone: Vec<Item> = lock(&self.items).drain().map(|(_, i)| i).collect();
        lock(&self.passwords).clear();
        if !gone.is_empty() {
            gone.into_iter().for_each(|item| forget(item, true));
            self.changed();
        }
    }

    // ---- sudo ---------------------------------------------------------------------

    /// How to run sudo on the session: with `password` (checked first), with the one typed
    /// earlier, or without one if sudo allows.
    async fn sudo_mode(
        &self,
        session: SessionId,
        password: Option<String>,
    ) -> Result<SudoMode, Refusal> {
        if let Some(password) = password.map(Zeroizing::new) {
            let mode = SudoMode::Password(password.clone());
            let (sudo, input) = mode.prefix();
            let check = self.run(session, &format!("{sudo} -v"), input).await?;
            if !succeeded(&check) {
                return Err(Refusal::Password {
                    wrong: true,
                    message: Some(
                        last_error_line(&check).unwrap_or_else(i18n::sudo_wrong_password),
                    ),
                });
            }
            lock(&self.passwords).insert(session, password);
            return Ok(mode);
        }
        if let Some(password) = lock(&self.passwords).get(&session).cloned() {
            return Ok(SudoMode::Password(password));
        }
        let check = self.run(session, "sudo -n true", Vec::new()).await?;
        if succeeded(&check) {
            Ok(SudoMode::NoPassword)
        } else {
            Err(Refusal::Password {
                wrong: false,
                message: None,
            })
        }
    }

    async fn run(
        &self,
        session: SessionId,
        command: &str,
        stdin: Vec<u8>,
    ) -> Result<ExecOutput, Refusal> {
        let options = ExecOptions {
            timeout: SUDO_TIMEOUT,
            max_output: (MAX_SIZE + 1) as usize,
            stdin,
        };
        Ok(self.sessions.exec(session, command, options).await?)
    }

    async fn sudo_read(
        &self,
        session: SessionId,
        path: &str,
        mode: &SudoMode,
    ) -> Result<Vec<u8>, Refusal> {
        let (sudo, input) = mode.prefix();
        let output = self
            .run(session, &format!("{sudo} cat -- {}", quote(path)), input)
            .await?;
        if !succeeded(&output) {
            return Err(self.sudo_failure(session, mode, &output));
        }
        if output.stdout.total() > MAX_SIZE {
            return Err(Refusal::Failed {
                message: i18n::edit_too_large(MAX_SIZE / 1024 / 1024),
            });
        }
        let (head, tail) = output.stdout.parts();
        let mut data = head.to_vec();
        data.extend_from_slice(&tail);
        Ok(data)
    }

    async fn sudo_write(
        &self,
        session: SessionId,
        path: &str,
        data: &[u8],
        mode: &SudoMode,
    ) -> Result<(), Refusal> {
        let (sudo, mut input) = mode.prefix();
        input.extend_from_slice(data);
        let command = format!("{sudo} tee -- {} > /dev/null", quote(path));
        let output = self.run(session, &command, input).await?;
        if succeeded(&output) {
            Ok(())
        } else {
            Err(self.sudo_failure(session, mode, &output))
        }
    }

    /// What a failed sudo command means: a password that no longer works is forgotten.
    fn sudo_failure(&self, session: SessionId, mode: &SudoMode, output: &ExecOutput) -> Refusal {
        let message = last_error_line(output);
        let refused = message.as_deref().is_some_and(|m| {
            let m = m.to_lowercase();
            m.contains("password") || m.contains("incorrect") || m.contains("sorry")
        });
        if refused {
            if matches!(mode, SudoMode::Password(_)) {
                lock(&self.passwords).remove(&session);
            }
            return Refusal::Password {
                wrong: matches!(mode, SudoMode::Password(_)),
                message,
            };
        }
        Refusal::Failed {
            message: i18n::sudo_failed(message.unwrap_or_else(|| format!("{:?}", output.exit))),
        }
    }
}

/// Stops a file's watcher and removes its copy, unless `keep_unsent` and the copy holds a
/// save that did not reach the server (it is left for the user, and removed when old).
fn forget(item: Item, keep_unsent: bool) {
    if let Some(task) = item.task {
        task.abort();
    }
    if keep_unsent && item.info.state != EditState::Synced {
        return;
    }
    if let Some(dir) = Path::new(&item.info.local_path).parent() {
        let dir = dir.to_path_buf();
        std::thread::spawn(move || std::fs::remove_dir_all(dir));
    }
}

/// Removes folders under `root` older than `age` (copies earlier runs left behind).
fn remove_old(root: &Path, age: Duration) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|elapsed| elapsed >= age);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Looks at a file's copy and sends it once a save is over.
async fn watch(edits: Weak<Edits>, id: EditId) {
    let Some(mut live) = edits.upgrade().map(|e| e.sessions.live_changes()) else {
        return;
    };
    let mut previous: Option<LocalStamp> = None;
    let mut looks: u64 = 0;
    loop {
        let reconnected = tokio::select! {
            () = tokio::time::sleep(POLL) => false,
            changed = live.changed() => {
                if changed.is_err() {
                    return;
                }
                true
            }
        };
        looks += 1;
        let Some(edits) = edits.upgrade() else {
            return;
        };
        let (path, state, sent, failed, busy) = {
            let items = lock(&edits.items);
            let Some(item) = items.get(&id) else {
                return;
            };
            (
                PathBuf::from(&item.info.local_path),
                item.info.state,
                item.local,
                item.failed,
                item.busy,
            )
        };
        // Missing for a moment while some editors save (through a temporary file).
        let Some(now) = local_stamp(&path) else {
            previous = None;
            continue;
        };
        let stable = previous == Some(now);
        previous = Some(now);
        let due = match state {
            // The user decides first.
            EditState::Conflict | EditState::Denied | EditState::Uploading => false,
            // Up as soon as the session is back (and now and then, in case it was missed).
            EditState::Waiting => reconnected || looks.is_multiple_of(WAITING_RETRY),
            EditState::Failed => Some(now) != failed && stable,
            EditState::Synced => Some(now) != sent && stable,
        };
        if due && !busy {
            edits.upload(id, false).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(quote("/etc/nginx/nginx.conf"), "'/etc/nginx/nginx.conf'");
        assert_eq!(quote("it's here"), r"'it'\''s here'");
        assert_eq!(quote("$(rm -rf /)"), "'$(rm -rf /)'");
    }

    #[test]
    fn a_password_is_consumed_by_sudo() {
        let mode = SudoMode::Password(Zeroizing::new("s3cret".into()));
        let (sudo, input) = mode.prefix();
        assert!(
            sudo.contains("-k"),
            "sudo must ask even when it remembers the user"
        );
        assert!(sudo.contains("-S"));
        assert_eq!(input, b"s3cret\n");
        let (sudo, input) = SudoMode::NoPassword.prefix();
        assert_eq!(sudo, "sudo -n");
        assert!(input.is_empty());
    }

    #[test]
    fn removes_only_old_folders() {
        let root = std::env::temp_dir().join(format!("nexssh-edit-test-{}", util::random_id()));
        std::fs::create_dir_all(root.join("fresh")).unwrap();
        remove_old(&root, Duration::from_secs(3600));
        assert!(root.join("fresh").exists());
        remove_old(&root, Duration::ZERO);
        assert!(!root.join("fresh").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
