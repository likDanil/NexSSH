//! Files over SFTP on a session's SSH connection (russh-sftp): browse, create folders,
//! rename, delete, upload and download. A session opens one SFTP channel on first use
//! (see [`crate::SessionManager::sftp`]), so no second login is needed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use russh::client::Handle;
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::fs::{File, Metadata};
use russh_sftp::protocol::{FilePermissions, StatusCode};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{Error, Result};
use crate::i18n;
use crate::session::handler::ClientHandler;

/// Read size for downloads; the client pipelines several requests per read.
const CHUNK: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Dir,
    File,
    Link,
    Other,
}

/// A directory entry, as shown in a file list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    /// A symlink to a directory: it opens like one.
    pub link_to_dir: bool,
    pub size: u64,
    /// Seconds since the Unix epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<u64>,
    /// `rwxr-xr-x`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<String>,
}

impl Entry {
    fn is_dir_like(&self) -> bool {
        self.kind == EntryKind::Dir || self.link_to_dir
    }
}

/// Transferred and total bytes.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

pub struct Sftp {
    session: SftpSession,
}

impl Sftp {
    pub(crate) async fn open(handle: &Handle<ClientHandler>) -> Result<Sftp> {
        let channel = handle.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let session = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| Error::invalid(i18n::sftp_unavailable(e)))?;
        // Listing a large directory on a slow link can take a while.
        session.set_timeout(60);
        Ok(Sftp { session })
    }

    /// The absolute path of the login directory.
    pub async fn home(&self) -> Result<String> {
        self.resolve(".").await
    }

    /// The absolute, normalized form of `path`. Relative paths, and `~`, start at the
    /// login directory.
    pub async fn resolve(&self, path: &str) -> Result<String> {
        let on_server = match path.strip_prefix('~') {
            Some("") => ".".to_string(),
            Some(rest) if rest.starts_with('/') => format!(".{rest}"),
            _ => path.to_string(),
        };
        self.session
            .canonicalize(on_server)
            .await
            .map_err(|e| fail(path, e))
    }

    /// Entries of `dir`: folders first, then by name.
    pub async fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        let items = self.session.read_dir(dir).await.map_err(|e| fail(dir, e))?;
        let mut entries = Vec::new();
        for item in items {
            let name = item.file_name();
            let meta = item.metadata();
            let kind = kind_of(&meta);
            let link_to_dir = kind == EntryKind::Link
                && self
                    .session
                    .metadata(join(dir, &name))
                    .await
                    .is_ok_and(|m| m.is_dir());
            entries.push(Entry {
                name,
                kind,
                link_to_dir,
                size: meta.size.unwrap_or(0),
                modified: meta.mtime.map(u64::from),
                permissions: meta
                    .permissions
                    .map(|p| FilePermissions::from(p).to_string()),
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir_like()
                .cmp(&a.is_dir_like())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    pub async fn mkdir(&self, path: &str) -> Result<()> {
        if self.session.try_exists(path).await.unwrap_or(false) {
            return Err(Error::invalid(i18n::sftp_exists(path)));
        }
        self.session
            .create_dir(path)
            .await
            .map_err(|e| fail(path, e))
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        if self.session.try_exists(to).await.unwrap_or(false) {
            return Err(Error::invalid(i18n::sftp_exists(to)));
        }
        self.session
            .rename(from, to)
            .await
            .map_err(|e| fail(from, e))
    }

    /// Deletes a file, a symlink (not its target) or a whole directory.
    pub async fn remove(&self, path: &str) -> Result<()> {
        let meta = self
            .session
            .symlink_metadata(path)
            .await
            .map_err(|e| fail(path, e))?;
        if !meta.is_dir() {
            return self
                .session
                .remove_file(path)
                .await
                .map_err(|e| fail(path, e));
        }
        // Depth-first without recursion: files go first, directories once empty.
        let mut pending = vec![path.to_string()];
        let mut dirs = Vec::new();
        while let Some(dir) = pending.pop() {
            for item in self
                .session
                .read_dir(&dir)
                .await
                .map_err(|e| fail(&dir, e))?
            {
                let child = join(&dir, &item.file_name());
                if item.metadata().is_dir() {
                    pending.push(child);
                } else {
                    self.session
                        .remove_file(&child)
                        .await
                        .map_err(|e| fail(&child, e))?;
                }
            }
            dirs.push(dir);
        }
        for dir in dirs.iter().rev() {
            self.session
                .remove_dir(dir)
                .await
                .map_err(|e| fail(dir, e))?;
        }
        Ok(())
    }

    /// Downloads a file or a whole directory into `dest_dir` under a name that does not
    /// exist there yet, and returns the local path. Symlinks inside directories are
    /// skipped (they may loop). A cancelled or failed download leaves nothing behind.
    pub async fn download(
        &self,
        remote: &str,
        dest_dir: &Path,
        on_progress: &mut (dyn FnMut(Progress) + Send),
        cancel: &AtomicBool,
    ) -> Result<PathBuf> {
        let meta = self
            .session
            .metadata(remote)
            .await
            .map_err(|e| fail(remote, e))?;
        std::fs::create_dir_all(dest_dir).map_err(|e| local_fail(dest_dir, e))?;
        let target = reserve(dest_dir, &base_name(remote), meta.is_dir())
            .map_err(|e| local_fail(dest_dir, e))?;
        let result = if meta.is_dir() {
            self.download_dir(remote, &target, on_progress, cancel)
                .await
        } else {
            let mut progress = Progress {
                done: 0,
                total: meta.size.unwrap_or(0),
            };
            on_progress(progress);
            self.fetch(remote, &target, &mut progress, on_progress, cancel)
                .await
        };
        match result {
            Ok(()) => Ok(target),
            Err(e) => {
                let _ = if target.is_dir() {
                    std::fs::remove_dir_all(&target)
                } else {
                    std::fs::remove_file(&target)
                };
                Err(e)
            }
        }
    }

    async fn download_dir(
        &self,
        remote: &str,
        target: &Path,
        on_progress: &mut (dyn FnMut(Progress) + Send),
        cancel: &AtomicBool,
    ) -> Result<()> {
        // Plan first so the progress has a total.
        let mut files: Vec<(String, PathBuf, u64)> = Vec::new();
        let mut dirs = vec![target.to_path_buf()];
        let mut pending = vec![(remote.to_string(), target.to_path_buf())];
        while let Some((dir, local)) = pending.pop() {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            for item in self
                .session
                .read_dir(&dir)
                .await
                .map_err(|e| fail(&dir, e))?
            {
                let name = item.file_name();
                let meta = item.metadata();
                let (child, child_local) = (join(&dir, &name), local.join(safe_name(&name)));
                match kind_of(&meta) {
                    EntryKind::Dir => {
                        dirs.push(child_local.clone());
                        pending.push((child, child_local));
                    }
                    EntryKind::File => files.push((child, child_local, meta.size.unwrap_or(0))),
                    EntryKind::Link | EntryKind::Other => {}
                }
            }
        }
        for dir in &dirs {
            std::fs::create_dir_all(dir).map_err(|e| local_fail(dir, e))?;
        }
        let mut progress = Progress {
            done: 0,
            total: files.iter().map(|f| f.2).sum(),
        };
        on_progress(progress);
        for (remote, local, _) in &files {
            self.fetch(remote, local, &mut progress, on_progress, cancel)
                .await?;
        }
        Ok(())
    }

    async fn fetch(
        &self,
        remote: &str,
        local: &Path,
        progress: &mut Progress,
        on_progress: &mut (dyn FnMut(Progress) + Send),
        cancel: &AtomicBool,
    ) -> Result<()> {
        let mut file = self
            .session
            .open(remote)
            .await
            .map_err(|e| fail(remote, e))?;
        let mut out = tokio::fs::File::create(local)
            .await
            .map_err(|e| local_fail(local, e))?;
        let mut buf = vec![0u8; CHUNK];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let n = file.read(&mut buf).await.map_err(|e| fail_io(remote, e))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .await
                .map_err(|e| local_fail(local, e))?;
            progress.done += n as u64;
            on_progress(*progress);
        }
        out.flush().await.map_err(|e| local_fail(local, e))?;
        let _ = file.shutdown().await;
        Ok(())
    }

    /// Creates (or replaces) a remote file to upload into.
    pub async fn create(&self, path: &str) -> Result<Upload> {
        let file = self.session.create(path).await.map_err(|e| fail(path, e))?;
        Ok(Upload {
            file,
            path: path.to_string(),
        })
    }

    /// Removes a partly uploaded file (best effort).
    pub async fn discard(&self, path: &str) {
        let _ = self.session.remove_file(path).await;
    }
}

/// A remote file being written; data arrives in chunks (e.g. from the UI).
pub struct Upload {
    file: File,
    path: String,
}

impl Upload {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.file
            .write_all(data)
            .await
            .map_err(|e| fail_io(&self.path, e))
    }

    /// Waits for every write to be acknowledged and closes the file.
    pub async fn finish(mut self) -> Result<()> {
        self.file
            .shutdown()
            .await
            .map_err(|e| fail_io(&self.path, e))
    }
}

fn kind_of(meta: &Metadata) -> EntryKind {
    if meta.is_dir() {
        EntryKind::Dir
    } else if meta.is_symlink() {
        EntryKind::Link
    } else if meta.is_regular() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

/// Where downloads go: the user's Downloads folder (or home).
pub fn download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir)
}

/// `dir/name` for SFTP (always `/`).
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The last path component; `/` for the root.
pub fn base_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit('/').next() {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => "root".to_string(),
    }
}

/// A remote name as a local file name: characters Windows does not allow are replaced.
fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        "_".to_string()
    } else {
        cleaned
    }
}

/// Creates the file (or folder) `dir/name`, or `name (1).ext`, `(2)`… when taken, and
/// returns its path. Creating it rather than checking keeps parallel downloads apart.
fn reserve(dir: &Path, name: &str, folder: bool) -> std::io::Result<PathBuf> {
    let name = safe_name(name);
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !folder && !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (name.clone(), String::new()),
    };
    let candidates = std::iter::once(dir.join(&name))
        .chain((1..).map(|i| dir.join(format!("{stem} ({i}){ext}"))));
    for candidate in candidates {
        let created = if folder {
            std::fs::create_dir(&candidate)
        } else {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
                .map(drop)
        };
        match created {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            other => return other.map(|()| candidate),
        }
    }
    unreachable!("the numbered names never run out")
}

fn fail(path: &str, e: SftpError) -> Error {
    let message = match &e {
        SftpError::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => i18n::sftp_no_such_file(path),
            StatusCode::PermissionDenied => i18n::sftp_permission_denied(path),
            StatusCode::NoConnection | StatusCode::ConnectionLost => i18n::not_connected(),
            _ if !status.error_message.trim().is_empty() => {
                i18n::sftp_failed(path, status.error_message.trim())
            }
            code => i18n::sftp_failed(path, code),
        },
        other => i18n::sftp_failed(path, other),
    };
    Error::Invalid(message)
}

fn fail_io(path: &str, e: std::io::Error) -> Error {
    Error::Invalid(i18n::sftp_failed(path, e))
}

fn local_fail(path: &Path, e: std::io::Error) -> Error {
    Error::Invalid(i18n::local_write_failed(&path.display().to_string(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_names() {
        assert_eq!(join("/home/u", "a.txt"), "/home/u/a.txt");
        assert_eq!(join("/", "etc"), "/etc");
        assert_eq!(base_name("/var/log/"), "log");
        assert_eq!(base_name("/"), "root");
        assert_eq!(safe_name("a:b?.txt"), "a_b_.txt");
        assert_eq!(safe_name("trailing. "), "trailing");
    }

    #[test]
    fn downloads_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("nexssh-unique-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("log.txt"), b"x").unwrap();
        std::fs::write(dir.join("log (1).txt"), b"x").unwrap();
        let take = |name, folder| reserve(&dir, name, folder).unwrap();
        assert_eq!(take("log.txt", false), dir.join("log (2).txt"));
        // A name is taken as soon as it is handed out.
        assert_eq!(take("log.txt", false), dir.join("log (3).txt"));
        assert_eq!(take("new.txt", false), dir.join("new.txt"));
        assert_eq!(take(".bashrc", false), dir.join(".bashrc"));
        assert_eq!(take(".bashrc", false), dir.join(".bashrc (1)"));
        assert_eq!(take("v1.2", true), dir.join("v1.2"));
        assert_eq!(take("v1.2", true), dir.join("v1.2 (1)"));
        assert!(dir.join("v1.2 (1)").is_dir());
        assert_eq!(std::fs::read(dir.join("log.txt")).unwrap(), b"x");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
