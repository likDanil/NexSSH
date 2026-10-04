//! Files over SFTP on a session's SSH connection (russh-sftp): browse, create files and
//! folders, rename, delete, change permissions, upload and download (files and whole
//! folders). A session opens one SFTP channel on first use (see
//! [`crate::SessionManager::sftp`]), so no second login is needed.
//!
//! Transfers keep many requests in flight, like OpenSSH's sftp: waiting for each reply
//! before the next request would make every chunk cost a round trip, and the speed would
//! be the round trip's, not the link's. How much is in flight follows the speed measured so
//! far (see [`Flow`]). Folders move several files at a time, and are listed, created,
//! deleted and changed several requests at a time, so small files do not cost a few round
//! trips one after another either.

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use futures_util::stream::{self, FuturesOrdered, FuturesUnordered, StreamExt};
use russh::client::Handle;
use russh_sftp::client::RawSftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::rawsession::{Limits, SftpResult};
use russh_sftp::extensions;
use russh_sftp::protocol::{FileAttributes, FilePermissions, OpenFlags, Status, StatusCode};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::error::{Error, Result};
use crate::i18n;
use crate::session::handler::ClientHandler;

/// Permission bits, including setuid, setgid and sticky (the rest of `st_mode` is the type).
const MODE_BITS: u32 = 0o7777;

/// Read and write size when the server does not announce its limits: what OpenSSH's sftp
/// uses then, and what every server takes.
const DEFAULT_IO_LEN: u64 = 32 * 1024;
/// The most asked for in one read or write, even where a server allows more (as OpenSSH's
/// sftp does).
const MAX_IO_LEN: u64 = 255 * 1024;
/// Bytes around the data of a data reply, and of a write request (with a handle of the
/// longest length the protocol allows).
const READ_OVERHEAD: u64 = 13;
const WRITE_OVERHEAD: u64 = 25 + 256;
/// What a server may take in one packet when it does not say (OpenSSH's limit).
const DEFAULT_PACKET_LEN: u64 = 256 * 1024;

/// Bytes a transfer keeps in flight at first, and at most.
const MIN_IN_FLIGHT: u64 = 256 * 1024;
const MAX_IN_FLIGHT: u64 = 8 * 1024 * 1024;
/// In flight: this much time's worth of data at the measured speed.
const IN_FLIGHT_TIME: Duration = Duration::from_millis(500);
/// The speed is measured over this much of the recent past.
const SPEED_WINDOW: Duration = Duration::from_secs(1);

/// Files of a folder moved at the same time.
const PARALLEL_FILES: usize = 16;
/// Small requests (listings, deletes, permission changes) of one operation at the same time.
const PARALLEL_REQUESTS: usize = 16;
/// Local reads for uploads.
const LOCAL_READ: usize = 1024 * 1024;
/// How often a transfer reports its progress.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

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
    /// The permission bits as a number (`0o755`), for editing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
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
    raw: Arc<RawSftpSession>,
    /// The most to ask for in one read, and to send in one write.
    read_len: u64,
    write_len: u64,
    /// Files of a folder moved at the same time (fewer if the server allows few open files).
    parallel: usize,
}

impl Sftp {
    pub(crate) async fn open(handle: &Handle<ClientHandler>) -> Result<Sftp> {
        let channel = handle.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let mut raw = RawSftpSession::new(channel.into_stream());
        let version = raw
            .init()
            .await
            .map_err(|e| Error::invalid(i18n::sftp_unavailable(e)))?;
        let (mut read_len, mut write_len) = (DEFAULT_IO_LEN, DEFAULT_IO_LEN);
        let mut parallel = PARALLEL_FILES;
        let has_limits = version
            .extensions
            .get(extensions::LIMITS)
            .is_some_and(|v| v == "1");
        if has_limits {
            match raw.limits().await {
                Ok(announced) => {
                    let limits = Limits::from(announced);
                    raw.set_limits(limits);
                    let packet = limits.packet_len.unwrap_or(DEFAULT_PACKET_LEN);
                    let fit = |len: Option<u64>, overhead: u64| {
                        len.unwrap_or(packet)
                            .min(packet.saturating_sub(overhead))
                            .clamp(1024, MAX_IO_LEN)
                    };
                    read_len = fit(limits.read_len, READ_OVERHEAD);
                    write_len = fit(limits.write_len, WRITE_OVERHEAD);
                    if let Some(handles) = limits.open_handles {
                        parallel = (handles / 2).clamp(1, PARALLEL_FILES as u64) as usize;
                    }
                }
                Err(e) => log::warn!("the SFTP server announced limits but did not send them: {e}"),
            }
        }
        // Listing a large directory on a slow link can take a while.
        raw.set_timeout(60);
        Ok(Sftp {
            raw: Arc::new(raw),
            read_len,
            write_len,
            parallel,
        })
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
        let name = self
            .raw
            .realpath(on_server)
            .await
            .map_err(|e| fail(path, e))?;
        name.files
            .into_iter()
            .next()
            .map(|f| f.filename)
            .ok_or_else(|| Error::invalid(i18n::sftp_no_such_file(path)))
    }

    /// Entries of `dir`: folders first, then by name.
    pub async fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        let items = self.read_dir(dir).await.map_err(|e| fail(dir, e))?;
        // Where symlinks point decides how they open: asked for all at once.
        let links: Vec<Option<String>> = items
            .iter()
            .map(|(name, meta)| (kind_of(meta) == EntryKind::Link).then(|| join(dir, name)))
            .collect();
        let link_to_dir: Vec<bool> = stream::iter(links)
            .map(|link| async move {
                match link {
                    Some(path) => self.raw.stat(path).await.is_ok_and(|a| a.attrs.is_dir()),
                    None => false,
                }
            })
            .buffered(PARALLEL_REQUESTS)
            .collect()
            .await;
        let mut entries: Vec<Entry> = items
            .into_iter()
            .zip(link_to_dir)
            .map(|((name, meta), link_to_dir)| Entry {
                name,
                kind: kind_of(&meta),
                link_to_dir,
                size: meta.size.unwrap_or(0),
                modified: meta.mtime.map(u64::from),
                permissions: meta
                    .permissions
                    .map(|p| FilePermissions::from(p).to_string()),
                mode: meta.permissions.map(|p| p & MODE_BITS),
            })
            .collect();
        entries.sort_by(|a, b| {
            b.is_dir_like()
                .cmp(&a.is_dir_like())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    pub async fn mkdir(&self, path: &str) -> Result<()> {
        if self.exists(path).await {
            return Err(Error::invalid(i18n::sftp_exists(path)));
        }
        self.raw
            .mkdir(path, FileAttributes::empty())
            .await
            .map(drop)
            .map_err(|e| fail(path, e))
    }

    /// Creates the folder unless it already exists (an upload merges into it).
    pub async fn ensure_dir(&self, path: &str) -> Result<()> {
        // Usually the folder is new: creating it right away saves a round trip.
        let Err(e) = self.raw.mkdir(path, FileAttributes::empty()).await else {
            return Ok(());
        };
        match self.raw.stat(path).await {
            Ok(found) if found.attrs.is_dir() => Ok(()),
            Ok(_) => Err(Error::invalid(i18n::sftp_exists(path))),
            Err(_) => Err(fail(path, e)),
        }
    }

    /// Creates the folders that do not exist yet (they merge into existing ones). A folder
    /// may be inside another one of them: parents are created first.
    pub async fn ensure_dirs(&self, paths: &[String]) -> Result<()> {
        let mut levels: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for path in paths {
            let depth = path.split('/').filter(|p| !p.is_empty()).count();
            levels.entry(depth).or_default().push(path.clone());
        }
        for level in levels.into_values() {
            self.each(level, |path| async move { self.ensure_dir(&path).await })
                .await?;
        }
        Ok(())
    }

    /// Creates an empty file; never replaces an existing one.
    pub async fn new_file(&self, path: &str) -> Result<()> {
        if self.exists(path).await {
            return Err(Error::invalid(i18n::sftp_exists(path)));
        }
        let flags = OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE;
        let handle = self
            .raw
            .open(path, flags, FileAttributes::empty())
            .await
            .map_err(|e| fail(path, e))?
            .handle;
        self.raw
            .close(handle)
            .await
            .map(drop)
            .map_err(|e| fail(path, e))
    }

    /// Sets the permission bits of `path`. With `recursive`, everything inside a folder gets
    /// them too, and folders also get execute where they have read (like `chmod -R a+X`), so
    /// that `644` leaves the folders openable. Symlinks inside are left alone.
    pub async fn chmod(&self, path: &str, mode: u32, recursive: bool) -> Result<()> {
        let meta = self.raw.lstat(path).await.map_err(|e| fail(path, e))?.attrs;
        if !recursive || !meta.is_dir() {
            return self.set_mode(path, mode & MODE_BITS).await;
        }
        // Everything is listed before anything changes (a new mode may close a folder to
        // listing), then files go first and folders deepest first, so the way in stays open
        // as long as it is needed.
        let found = self.walk(path, None).await?;
        let files: Vec<String> = found
            .iter()
            .filter(|f| kind_of(&f.meta) == EntryKind::File)
            .map(|f| f.path.clone())
            .collect();
        self.each(files, |file| async move {
            self.set_mode(&file, mode & MODE_BITS).await
        })
        .await?;
        for level in folders_deepest_first(path, &found) {
            self.each(level, |dir| async move {
                self.set_mode(&dir, folder_mode(mode)).await
            })
            .await?;
        }
        Ok(())
    }

    async fn set_mode(&self, path: &str, mode: u32) -> Result<()> {
        let attrs = FileAttributes {
            permissions: Some(mode),
            ..FileAttributes::empty()
        };
        self.raw
            .setstat(path, attrs)
            .await
            .map(drop)
            .map_err(|e| fail(path, e))
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        if self.exists(to).await {
            return Err(Error::invalid(i18n::sftp_exists(to)));
        }
        self.raw
            .rename(from, to)
            .await
            .map(drop)
            .map_err(|e| fail(from, e))
    }

    /// Deletes a file, a symlink (not its target) or a whole directory.
    pub async fn remove(&self, path: &str) -> Result<()> {
        let meta = self.raw.lstat(path).await.map_err(|e| fail(path, e))?.attrs;
        if !meta.is_dir() {
            return self.remove_file(path).await;
        }
        // Files all at once, then folders deepest first, each once it is empty.
        let found = self.walk(path, None).await?;
        let files: Vec<String> = found
            .iter()
            .filter(|f| !f.meta.is_dir())
            .map(|f| f.path.clone())
            .collect();
        self.each(files, |file| async move { self.remove_file(&file).await })
            .await?;
        for level in folders_deepest_first(path, &found) {
            self.each(level, |dir| async move {
                self.raw
                    .rmdir(dir.as_str())
                    .await
                    .map(drop)
                    .map_err(|e| fail(&dir, e))
            })
            .await?;
        }
        Ok(())
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        self.raw
            .remove(path)
            .await
            .map(drop)
            .map_err(|e| fail(path, e))
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
            .raw
            .stat(remote)
            .await
            .map_err(|e| fail(remote, e))?
            .attrs;
        std::fs::create_dir_all(dest_dir).map_err(|e| local_fail(dest_dir, e))?;
        let target = reserve(dest_dir, &base_name(remote), meta.is_dir())
            .map_err(|e| local_fail(dest_dir, e))?;
        let job = Job::new(cancel);
        let result = if meta.is_dir() {
            self.download_dir(&job, remote, &target, on_progress).await
        } else {
            let size = meta.size.unwrap_or(0);
            let one = std::iter::once(self.fetch(&job, remote, &target, size));
            job.run(size, 1, one, on_progress).await
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
        job: &Job<'_>,
        remote: &str,
        target: &Path,
        on_progress: &mut (dyn FnMut(Progress) + Send),
    ) -> Result<()> {
        // Plan first so the progress has a total. Folders come before what is inside them.
        let mut files: Vec<(String, PathBuf, u64)> = Vec::new();
        for item in self.walk(remote, Some(job)).await? {
            let local = item.names.iter().fold(target.to_path_buf(), |path, name| {
                path.join(safe_name(name))
            });
            match kind_of(&item.meta) {
                EntryKind::Dir => {
                    std::fs::create_dir_all(&local).map_err(|e| local_fail(&local, e))?
                }
                EntryKind::File => files.push((item.path, local, item.meta.size.unwrap_or(0))),
                EntryKind::Link | EntryKind::Other => {}
            }
        }
        let total = files.iter().map(|f| f.2).sum();
        // Owned paths: a closure taking references here would keep the future from being
        // `Send` (a limitation of the compiler).
        let fetches = files.into_iter().map(|(remote, local, size)| async move {
            self.fetch(job, &remote, &local, size).await
        });
        job.run(total, self.parallel, fetches, on_progress).await
    }

    /// Downloads one file into `local`.
    async fn fetch(&self, job: &Job<'_>, remote: &str, local: &Path, size: u64) -> Result<()> {
        job.check()?;
        let mut out = tokio::fs::File::create(local)
            .await
            .map_err(|e| local_fail(local, e))?;
        let received = self.receive(job, remote, size, &mut out, local).await;
        // The last write finishes either way: a failed download is removed, and Windows
        // cannot remove a file that is still open.
        let flushed = out.flush().await.map_err(|e| local_fail(local, e));
        received.and(flushed)
    }

    /// Reads a remote file into `out`: reads ask for what the size says is there, plus one
    /// at the end that finds the end (or that the file grew), all at once as far as the
    /// transfer's flow allows.
    async fn receive(
        &self,
        job: &Job<'_>,
        remote: &str,
        size: u64,
        out: &mut tokio::fs::File,
        local: &Path,
    ) -> Result<()> {
        let file = self.open_file(remote, OpenFlags::READ).await?;
        let mut lane = Lane::new(&job.flow);
        let read = |offset: u64, len: u64| {
            let (raw, handle) = (&self.raw, file.handle.clone());
            async move { (offset, len, raw.read(handle, offset, len as u32).await) }
        };
        let mut queue = FuturesOrdered::new();
        let mut chunk = self.read_len;
        // Reads are planned up to `end`; once the file turns out longer than that, they go
        // on until the end is found.
        let (mut end, mut open_ended, mut end_asked) = (size, false, false);
        let mut next = 0;
        loop {
            job.check()?;
            while lane.has_room() {
                let len = if open_ended {
                    chunk
                } else if next < end {
                    chunk.min(end - next)
                } else if !end_asked {
                    end_asked = true;
                    chunk
                } else {
                    break;
                };
                queue.push_back(read(next, len));
                lane.sent(len);
                next += len;
            }
            let Some((offset, asked, reply)) = queue.next().await else {
                break;
            };
            lane.done(asked);
            let mut data = match reply {
                Ok(data) if !data.data.is_empty() => data.data,
                Ok(_) => break,
                Err(SftpError::Status(s)) if s.status_code == StatusCode::Eof => break,
                Err(e) => return Err(fail(remote, e)),
            };
            // More than asked for would overlap the next read.
            data.truncate(asked as usize);
            out.write_all(&data)
                .await
                .map_err(|e| local_fail(local, e))?;
            let got = data.len() as u64;
            job.flow.moved(got);
            if got < asked {
                // A short read: what was asked for after it no longer lines up, so it is
                // dropped and the rest is asked for again from here.
                queue = FuturesOrdered::new();
                lane.dropped();
                next = offset + got;
                if next < end {
                    // The server reads less at a time than asked.
                    chunk = got;
                }
                end = end.max(next);
                end_asked = false;
            } else if offset + got > end {
                open_ended = true;
            }
        }
        Ok(())
    }

    /// Uploads a local file or folder into `remote_dir` under its own name and returns the
    /// remote path. Folders merge into existing ones and files replace existing files (the
    /// UI asks first). Inside folders, symlinks to files are followed and symlinks to
    /// folders skipped (they may loop). A cancelled or failed upload removes the files it was
    /// writing; what was uploaded before stays.
    pub async fn upload_path(
        &self,
        local: &Path,
        remote_dir: &str,
        on_progress: &mut (dyn FnMut(Progress) + Send),
        cancel: &AtomicBool,
    ) -> Result<String> {
        let unreadable = |path: &Path, e| local_read_fail(path, e);
        let meta = std::fs::metadata(local).map_err(|e| unreadable(local, e))?;
        let name = local
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| Error::invalid(i18n::sftp_no_such_file(&local.display().to_string())))?;
        let target = join(remote_dir, &name);
        let job = &Job::new(cancel);

        // Plan first so the progress has a total.
        let mut dirs = Vec::new();
        let mut files: Vec<(PathBuf, String, u64)> = Vec::new();
        if meta.is_dir() {
            dirs.push(target.clone());
            let mut pending = vec![(local.to_path_buf(), target.clone())];
            while let Some((dir, remote)) = pending.pop() {
                job.check()?;
                for item in std::fs::read_dir(&dir).map_err(|e| unreadable(&dir, e))? {
                    let item = item.map_err(|e| unreadable(&dir, e))?;
                    let (path, child) = (
                        item.path(),
                        join(&remote, &item.file_name().to_string_lossy()),
                    );
                    let kind = item.file_type().map_err(|e| unreadable(&path, e))?;
                    if kind.is_dir() {
                        dirs.push(child.clone());
                        pending.push((path, child));
                    } else if let Ok(meta) = std::fs::metadata(&path)
                        && meta.is_file()
                    {
                        files.push((path, child, meta.len()));
                    }
                }
            }
        } else {
            files.push((local.to_path_buf(), target.clone(), meta.len()));
        }

        let total = files.iter().map(|f| f.2).sum();
        on_progress(Progress { done: 0, total });
        job.check()?;
        self.ensure_dirs(&dirs).await?;
        let sends = files
            .into_iter()
            .map(|(path, remote, _)| async move { self.send(job, &path, &remote).await });
        job.run(total, self.parallel, sends, on_progress).await?;
        Ok(target)
    }

    /// Uploads one local file to `remote`.
    async fn send(&self, job: &Job<'_>, local: &Path, remote: &str) -> Result<()> {
        job.check()?;
        let input = tokio::fs::File::open(local)
            .await
            .map_err(|e| local_read_fail(local, e))?;
        let mut input = BufReader::with_capacity(LOCAL_READ, input);
        let mut writer = self.create_writer(remote, Arc::clone(&job.flow)).await?;
        let sent: Result<()> = async {
            let mut piece = vec![0u8; self.write_len as usize];
            loop {
                job.check()?;
                let n = fill(&mut input, &mut piece)
                    .await
                    .map_err(|e| local_read_fail(local, e))?;
                if n == 0 {
                    return Ok(());
                }
                writer.push(piece[..n].to_vec()).await?;
            }
        }
        .await;
        match sent {
            Ok(()) => writer.finish().await,
            Err(e) => {
                writer.abandon().await;
                Err(e)
            }
        }
    }

    /// Creates (or replaces) a remote file to upload into.
    pub async fn create(&self, path: &str) -> Result<Upload> {
        let writer = self.create_writer(path, Arc::new(Flow::default())).await?;
        Ok(Upload {
            writer,
            write_len: self.write_len as usize,
        })
    }

    async fn create_writer(&self, path: &str, flow: Arc<Flow>) -> Result<Writer> {
        let flags = OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE;
        let handle = self
            .raw
            .open(path, flags, FileAttributes::empty())
            .await
            .map_err(|e| fail(path, e))?
            .handle;
        Ok(Writer {
            raw: Arc::clone(&self.raw),
            handle: Some(handle),
            path: path.to_string(),
            offset: 0,
            replies: FuturesUnordered::new(),
            mine: 0,
            flow,
        })
    }

    // ---- small files, whole in memory -----------------------------------------------

    /// Up to `len` bytes of the file `path` from `offset`, and the file's size as the server
    /// gives it (if it does; files such as those in `/proc` say 0, whatever they hold).
    pub async fn read_part(
        &self,
        path: &str,
        offset: u64,
        len: u64,
    ) -> Result<(Vec<u8>, Option<u64>)> {
        let meta = self.raw.stat(path).await.map_err(|e| fail(path, e))?.attrs;
        if meta.is_dir() {
            return Err(Error::invalid(i18n::sftp_is_folder(path)));
        }
        let file = self.open_file(path, OpenFlags::READ).await?;
        let mut data = Vec::new();
        while (data.len() as u64) < len {
            let want = (len - data.len() as u64).min(self.read_len);
            let at = offset + data.len() as u64;
            match self.raw.read(file.handle.as_str(), at, want as u32).await {
                Ok(reply) if !reply.data.is_empty() => {
                    let got = reply.data.len().min(want as usize);
                    data.extend_from_slice(&reply.data[..got]);
                }
                Ok(_) => break,
                Err(SftpError::Status(s)) if s.status_code == StatusCode::Eof => break,
                Err(e) => return Err(fail(path, e)),
            }
        }
        Ok((data, meta.size))
    }

    /// Makes `data` the whole content of the file `path`, creating it or replacing what it
    /// held (its permissions stay). Unlike an upload, a write that fails does not remove the
    /// file: it may be one the user had (a configuration file being edited).
    pub async fn write_whole(&self, path: &str, data: &[u8]) -> Result<()> {
        let flags = OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE;
        let handle = self
            .raw
            .open(path, flags, FileAttributes::empty())
            .await
            .map_err(|e| fail(path, e))?
            .handle;
        // All at once: the content is small (it came in one message).
        let len = self.write_len as usize;
        let writes = data.chunks(len).enumerate().map(|(i, piece)| {
            self.raw
                .write(handle.as_str(), (i * len) as u64, piece.to_vec())
        });
        let written: Vec<_> = futures_util::future::join_all(writes).await;
        let closed = self.raw.close(handle).await;
        for reply in written {
            reply.map_err(|e| fail(path, e))?;
        }
        closed.map(drop).map_err(|e| fail(path, e))
    }

    // ---- requests ---------------------------------------------------------------------

    /// Whether `path` is known to exist (if the server cannot say, the operation itself will).
    async fn exists(&self, path: &str) -> bool {
        self.raw.stat(path).await.is_ok()
    }

    async fn open_file(&self, path: &str, flags: OpenFlags) -> Result<OpenFile> {
        let handle = self
            .raw
            .open(path, flags, FileAttributes::empty())
            .await
            .map_err(|e| fail(path, e))?
            .handle;
        Ok(OpenFile {
            raw: Arc::clone(&self.raw),
            handle,
        })
    }

    /// The entries of a folder (without `.` and `..`), with their attributes as `lstat`
    /// gives them.
    async fn read_dir(&self, path: &str) -> SftpResult<Vec<(String, FileAttributes)>> {
        let dir = OpenFile {
            raw: Arc::clone(&self.raw),
            handle: self.raw.opendir(path).await?.handle,
        };
        let mut entries = Vec::new();
        loop {
            match self.raw.readdir(dir.handle.as_str()).await {
                Ok(name) => entries.extend(
                    name.files
                        .into_iter()
                        .filter(|f| f.filename != "." && f.filename != "..")
                        .map(|f| (f.filename, f.attrs)),
                ),
                Err(SftpError::Status(s)) if s.status_code == StatusCode::Eof => {
                    return Ok(entries);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Everything inside the folder `root`, at any depth (symlinks are not followed).
    /// Folders are listed several at a time.
    async fn walk(&self, root: &str, job: Option<&Job<'_>>) -> Result<Vec<Found>> {
        let mut found = Vec::new();
        let mut waiting: Vec<(String, Vec<String>)> = vec![(root.to_string(), Vec::new())];
        let mut listing = FuturesUnordered::new();
        loop {
            while listing.len() < PARALLEL_REQUESTS
                && let Some((dir, names)) = waiting.pop()
            {
                listing.push(async move {
                    let entries = self.read_dir(&dir).await;
                    (dir, names, entries)
                });
            }
            let Some((dir, names, entries)) = listing.next().await else {
                return Ok(found);
            };
            if let Some(job) = job {
                job.check()?;
            }
            for (name, meta) in entries.map_err(|e| fail(&dir, e))? {
                let path = join(&dir, &name);
                let mut child = names.clone();
                child.push(name);
                if meta.is_dir() {
                    waiting.push((path.clone(), child.clone()));
                }
                found.push(Found {
                    path,
                    names: child,
                    meta,
                });
            }
        }
    }

    /// Runs `task` for every path, several at a time; the first failure is the result. (The
    /// paths are owned: a task taking references would keep the future from being `Send`.)
    async fn each<F, Fut>(&self, paths: Vec<String>, task: F) -> Result<()>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let mut running = stream::iter(paths)
            .map(task)
            .buffer_unordered(PARALLEL_REQUESTS);
        while let Some(result) = running.next().await {
            result?;
        }
        Ok(())
    }
}

/// An entry found inside a folder by [`Sftp::walk`].
struct Found {
    path: String,
    /// The names from the walked folder down to the entry.
    names: Vec<String>,
    meta: FileAttributes,
}

/// The folders in `found`, deepest first, each depth together (`root` last).
fn folders_deepest_first(root: &str, found: &[Found]) -> Vec<Vec<String>> {
    let mut levels: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    levels.insert(0, vec![root.to_string()]);
    for item in found.iter().filter(|f| f.meta.is_dir()) {
        levels
            .entry(item.names.len())
            .or_default()
            .push(item.path.clone());
    }
    levels.into_values().rev().collect()
}

/// A remote file open for reading, or a folder open for listing: closed when dropped, in the
/// background (nothing waits for the reply).
struct OpenFile {
    raw: Arc<RawSftpSession>,
    handle: String,
}

impl Drop for OpenFile {
    fn drop(&mut self) {
        let handle = std::mem::take(&mut self.handle);
        let raw = Arc::clone(&self.raw);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = raw.close(handle).await;
            });
        }
    }
}

// ---- flow ---------------------------------------------------------------------------

/// What a transfer keeps in flight: about [`IN_FLIGHT_TIME`] of data at the speed of the
/// last [`SPEED_WINDOW`], at least [`MIN_IN_FLIGHT`] and at most [`MAX_IN_FLIGHT`]. That is
/// more than one round trip's worth, so the link stays busy; starting small and growing with
/// the measured speed fills a fast link within a few round trips without queueing so much
/// on a slow one that requests time out. The files of a folder share one flow.
#[derive(Default)]
struct Flow {
    in_flight: AtomicU64,
    /// Bytes moved so far.
    moved: AtomicU64,
    speed: Mutex<Speed>,
}

#[derive(Default)]
struct Speed {
    /// When the first request went out (planning before it does not count).
    started: Option<Instant>,
    /// Bytes moved within the window, and when.
    recent: VecDeque<(Instant, u64)>,
    recent_bytes: u64,
}

impl Flow {
    fn budget(&self) -> u64 {
        let mut speed = self.speed.lock().unwrap_or_else(|e| e.into_inner());
        let span = speed
            .started
            .get_or_insert_with(Instant::now)
            .elapsed()
            .clamp(Duration::from_millis(10), SPEED_WINDOW);
        let per_second = speed.recent_bytes as f64 / span.as_secs_f64();
        ((per_second * IN_FLIGHT_TIME.as_secs_f64()) as u64).clamp(MIN_IN_FLIGHT, MAX_IN_FLIGHT)
    }

    fn moved(&self, bytes: u64) {
        self.moved.fetch_add(bytes, Ordering::Relaxed);
        let mut speed = self.speed.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        speed.recent.push_back((now, bytes));
        speed.recent_bytes += bytes;
        while let Some(&(at, n)) = speed.recent.front() {
            if now.duration_since(at) <= SPEED_WINDOW {
                break;
            }
            speed.recent.pop_front();
            speed.recent_bytes -= n;
        }
    }
}

/// One file's share of a [`Flow`]: what it has in flight. A file may always have one
/// request out, so none waits for the others.
struct Lane<'a> {
    flow: &'a Flow,
    mine: u64,
}

impl<'a> Lane<'a> {
    fn new(flow: &'a Flow) -> Self {
        Lane { flow, mine: 0 }
    }

    fn has_room(&self) -> bool {
        self.mine == 0 || self.flow.in_flight.load(Ordering::Relaxed) < self.flow.budget()
    }

    fn sent(&mut self, bytes: u64) {
        self.mine += bytes;
        self.flow.in_flight.fetch_add(bytes, Ordering::Relaxed);
    }

    fn done(&mut self, bytes: u64) {
        self.mine -= bytes;
        self.flow.in_flight.fetch_sub(bytes, Ordering::Relaxed);
    }

    /// Everything out was dropped.
    fn dropped(&mut self) {
        self.done(self.mine);
    }
}

impl Drop for Lane<'_> {
    fn drop(&mut self) {
        self.dropped();
    }
}

/// One download or upload (of a file or a folder): its flow, and whether it should stop.
struct Job<'a> {
    flow: Arc<Flow>,
    cancel: &'a AtomicBool,
    /// Set by the first file that fails, so the others stop too.
    failed: AtomicBool,
}

impl<'a> Job<'a> {
    fn new(cancel: &'a AtomicBool) -> Self {
        Job {
            flow: Arc::new(Flow::default()),
            cancel,
            failed: AtomicBool::new(false),
        }
    }

    fn check(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) || self.failed.load(Ordering::Relaxed) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Runs the transfers of the files, `parallel` at a time, and reports the progress
    /// meanwhile. The first failure stops the others (each cleans up after itself) and is
    /// the result.
    async fn run<F>(
        &self,
        total: u64,
        parallel: usize,
        files: impl Iterator<Item = F>,
        on_progress: &mut (dyn FnMut(Progress) + Send),
    ) -> Result<()>
    where
        F: Future<Output = Result<()>>,
    {
        let mut running = stream::iter(files).buffer_unordered(parallel.max(1));
        let mut tick = tokio::time::interval(PROGRESS_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut first_error = None;
        let progress = || Progress {
            done: self.flow.moved.load(Ordering::Relaxed),
            total,
        };
        loop {
            tokio::select! {
                next = running.next() => match next {
                    Some(Ok(())) => {}
                    Some(Err(e)) => {
                        if first_error.is_none() {
                            self.failed.store(true, Ordering::Relaxed);
                            first_error = Some(e);
                        }
                    }
                    None => break,
                },
                _ = tick.tick() => on_progress(progress()),
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => {
                on_progress(progress());
                Ok(())
            }
        }
    }
}

// ---- writing ------------------------------------------------------------------------

type WriteReply = Pin<Box<dyn Future<Output = (u64, SftpResult<Status>)> + Send>>;

/// Writes into an open remote file with many writes in flight. Dropped before
/// [`Writer::finish`], it closes and removes the file in the background.
struct Writer {
    raw: Arc<RawSftpSession>,
    /// `None` once closed.
    handle: Option<String>,
    path: String,
    offset: u64,
    replies: FuturesUnordered<WriteReply>,
    /// Bytes in flight.
    mine: u64,
    flow: Arc<Flow>,
}

impl Writer {
    /// Writes `data` (at most one write's length) after what was written so far, first
    /// waiting until the flow has room for it.
    async fn push(&mut self, data: Vec<u8>) -> Result<()> {
        while self.mine > 0 && self.flow.in_flight.load(Ordering::Relaxed) >= self.flow.budget() {
            self.settle().await?;
        }
        let Some(handle) = self.handle.clone() else {
            return Err(Error::Cancelled);
        };
        let (raw, offset, len) = (Arc::clone(&self.raw), self.offset, data.len() as u64);
        self.replies.push(Box::pin(async move {
            (len, raw.write(handle, offset, data).await)
        }));
        self.offset += len;
        self.mine += len;
        self.flow.in_flight.fetch_add(len, Ordering::Relaxed);
        Ok(())
    }

    /// Waits for one reply.
    async fn settle(&mut self) -> Result<()> {
        match self.replies.next().await {
            Some(reply) => self.take(reply),
            None => Ok(()),
        }
    }

    /// Sends the writes queued so far and takes the replies already here, without waiting.
    fn send_queued(&mut self) -> Result<()> {
        while let Some(Some(reply)) = self.replies.next().now_or_never() {
            self.take(reply)?;
        }
        Ok(())
    }

    fn take(&mut self, (len, reply): (u64, SftpResult<Status>)) -> Result<()> {
        self.mine -= len;
        self.flow.in_flight.fetch_sub(len, Ordering::Relaxed);
        reply.map_err(|e| fail(&self.path, e))?;
        if len > 0 {
            self.flow.moved(len);
        }
        Ok(())
    }

    /// Waits for every write and closes the file; on failure the file is removed. The close
    /// goes right behind the writes, without waiting for them: the server takes a file's
    /// requests in order, so it closes the file complete, and a small file costs one round
    /// trip less.
    async fn finish(mut self) -> Result<()> {
        if let Some(handle) = self.handle.take() {
            let raw = Arc::clone(&self.raw);
            self.replies
                .push(Box::pin(async move { (0, raw.close(handle).await) }));
        }
        let mut result = Ok(());
        while !self.replies.is_empty() {
            // Every reply is waited for, the close's too, before anything is removed.
            result = result.and(self.settle().await);
        }
        if result.is_err() {
            self.abandon().await;
        }
        result
    }

    /// Gives up: closes the file and removes it.
    async fn abandon(mut self) {
        self.forget_replies();
        if let Some(handle) = self.handle.take() {
            let _ = self.raw.close(handle).await;
        }
        let _ = self.raw.remove(self.path.as_str()).await;
    }

    fn forget_replies(&mut self) {
        self.replies = FuturesUnordered::new();
        self.flow.in_flight.fetch_sub(self.mine, Ordering::Relaxed);
        self.mine = 0;
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.forget_replies();
        let Some(handle) = self.handle.take() else {
            return;
        };
        let (raw, path) = (Arc::clone(&self.raw), std::mem::take(&mut self.path));
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = raw.close(handle).await;
                let _ = raw.remove(path).await;
            });
        }
    }
}

/// A remote file being written; data arrives in pieces (e.g. from the UI). Writes are not
/// waited for one by one: a piece returns once it is on its way, unless much is in flight
/// already.
pub struct Upload {
    writer: Writer,
    write_len: usize,
}

impl Upload {
    pub async fn write(&mut self, data: &[u8]) -> Result<()> {
        for piece in data.chunks(self.write_len) {
            self.writer.push(piece.to_vec()).await?;
        }
        self.writer.send_queued()
    }

    /// Waits for every write to be acknowledged and closes the file. A failed upload
    /// removes the file.
    pub async fn finish(self) -> Result<()> {
        self.writer.finish().await
    }

    /// Abandons the upload: closes and removes the file.
    pub async fn cancel(self) {
        self.writer.abandon().await
    }
}

/// Reads until `buf` is full or the input ends; returns how much was read.
async fn fill(input: &mut (impl AsyncRead + Unpin), buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match input.read(&mut buf[filled..]).await? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// `mode` for a folder in a recursive change: execute (enter) wherever there is read.
fn folder_mode(mode: u32) -> u32 {
    let mode = mode & MODE_BITS;
    mode | ((mode & 0o444) >> 2)
}

fn kind_of(meta: &FileAttributes) -> EntryKind {
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

fn local_fail(path: &Path, e: std::io::Error) -> Error {
    Error::Invalid(i18n::local_write_failed(&path.display().to_string(), e))
}

fn local_read_fail(path: &Path, e: std::io::Error) -> Error {
    Error::Invalid(i18n::local_read_failed(&path.display().to_string(), e))
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
    fn folders_stay_openable_in_recursive_chmod() {
        assert_eq!(folder_mode(0o644), 0o755);
        assert_eq!(folder_mode(0o640), 0o750);
        assert_eq!(folder_mode(0o600), 0o700);
        assert_eq!(folder_mode(0o000), 0o000);
        assert_eq!(folder_mode(0o1777), 0o1777);
        assert_eq!(folder_mode(0o100644), 0o755, "type bits are dropped");
    }

    #[test]
    fn in_flight_follows_the_speed() {
        let flow = Flow::default();
        assert_eq!(flow.budget(), MIN_IN_FLIGHT, "small until measured");
        std::thread::sleep(Duration::from_millis(20));
        // About 100 MiB/s: capped.
        flow.moved(2 * 1024 * 1024);
        assert_eq!(flow.budget(), MAX_IN_FLIGHT);

        // A slow link never gets more queued than a fraction of a second's worth.
        let slow = Flow::default();
        slow.budget();
        std::thread::sleep(Duration::from_millis(200));
        slow.moved(400 * 1024);
        let budget = slow.budget();
        assert!((MIN_IN_FLIGHT..=1024 * 1024).contains(&budget), "{budget}");
    }

    #[test]
    fn deepest_folders_first() {
        let found = |path: &str, names: &[&str], dir: bool| {
            let mut meta = FileAttributes::empty();
            meta.set_dir(dir);
            Found {
                path: path.into(),
                names: names.iter().map(|n| n.to_string()).collect(),
                meta,
            }
        };
        let items = [
            found("/r/a", &["a"], true),
            found("/r/a/b", &["a", "b"], true),
            found("/r/a/f", &["a", "f"], false),
            found("/r/c", &["c"], true),
        ];
        let levels = folders_deepest_first("/r", &items);
        assert_eq!(
            levels,
            vec![vec!["/r/a/b"], vec!["/r/a", "/r/c"], vec!["/r"]]
        );
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
