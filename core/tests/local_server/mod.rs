//! An SSH server with SFTP in this process (russh + russh-sftp over a temp folder), behind a
//! proxy that adds network latency and can cap bandwidth: transfer tests and speed
//! measurements without an sshd. The SFTP side answers like OpenSSH's sftp-server: one
//! request at a time, `limits@openssh.com` (8.6+) or the 64 KiB reads of older versions.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nexssh_core::sftp::Sftp;
use nexssh_core::{AuthKind, Server, SessionStatus};
use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{Auth, ChannelOpenHandle, Msg, Session as ServerSession};
use russh::{Channel, ChannelId};
use russh_sftp::extensions::LimitsExtension;
use russh_sftp::protocol::{
    Attrs, Data, ExtendedReply, File, FileAttributes, Handle, Name, OpenFlags, Packet, Status,
    StatusCode, Version,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::common::{self, Session};

#[derive(Clone, Debug)]
pub struct Options {
    /// Added to every packet in each direction (half the round trip).
    pub delay: Duration,
    /// Bytes per second in each direction; `None` for no cap.
    pub bandwidth: Option<u64>,
    /// Announces `limits@openssh.com` like OpenSSH 8.6+.
    pub limits: bool,
    /// The largest read the server answers (OpenSSH: 255 KiB, before 8.6: 64 KiB).
    pub max_read: u32,
    /// Leaves sizes out of file attributes (like `/proc` files, which say 0).
    pub hide_sizes: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            delay: Duration::ZERO,
            bandwidth: None,
            limits: true,
            max_read: 255 * 1024,
            hide_sizes: false,
        }
    }
}

impl Options {
    /// An OpenSSH older than 8.6: no limits extension, reads of at most 64 KiB.
    pub fn old_openssh(mut self) -> Self {
        self.limits = false;
        self.max_read = 64 * 1024;
        self
    }
}

pub struct LocalServer {
    /// The folder served as `/`.
    pub root: PathBuf,
    /// Where to connect (the latency proxy).
    pub port: u16,
    /// A private key it accepts (it accepts any).
    pub key_file: PathBuf,
}

impl LocalServer {
    pub async fn start(options: Options) -> LocalServer {
        let dir = common::temp_dir();
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();

        let client_key =
            PrivateKey::random(&mut russh::keys::key::safe_rng(), Algorithm::Ed25519).unwrap();
        let key_file = dir.join("id_ed25519");
        let pem = client_key
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap();
        std::fs::write(&key_file, pem.as_bytes()).unwrap();

        let host_key =
            PrivateKey::random(&mut russh::keys::key::safe_rng(), Algorithm::Ed25519).unwrap();
        let config = Arc::new(russh::server::Config {
            keys: vec![host_key],
            auth_rejection_time: Duration::from_millis(10),
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let ssh = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ssh_addr = ssh.local_addr().unwrap();
        let served = root.clone();
        let sftp_options = options.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = ssh.accept().await {
                let _ = stream.set_nodelay(true);
                let handler = SshHandler {
                    root: served.clone(),
                    options: sftp_options.clone(),
                    channels: HashMap::new(),
                };
                let config = Arc::clone(&config);
                tokio::spawn(async move {
                    if let Ok(session) = russh::server::run_stream(config, stream, handler).await {
                        let _ = session.await;
                    }
                });
            }
        });

        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = proxy.local_addr().unwrap().port();
        tokio::spawn(run_proxy(proxy, ssh_addr, options.delay, options.bandwidth));
        LocalServer {
            root,
            port,
            key_file,
        }
    }

    pub fn server(&self) -> Server {
        Server {
            name: "local".into(),
            host: "127.0.0.1".into(),
            port: self.port,
            user: "tester".into(),
            auth: AuthKind::Key,
            identity_file: Some(self.key_file.display().to_string()),
            ..Server::default()
        }
    }

    /// Connects a fresh core and opens the session's SFTP client.
    pub async fn connect(&self) -> (Session, Arc<Sftp>) {
        let core = Arc::new(common::core());
        let mut session = Session::open(Arc::clone(&core), self.server());
        session.accept_host_key().await;
        session.expect_status(SessionStatus::Connected).await;
        let sftp = core.sessions.sftp(session.id).await.unwrap();
        (session, sftp)
    }

    /// A path on the server's disk for a remote path such as `/dir/file`.
    pub fn local(&self, remote: &str) -> PathBuf {
        let mut path = self.root.clone();
        for part in remote.split('/').filter(|p| !p.is_empty()) {
            path.push(part);
        }
        path
    }
}

// ---- latency proxy ------------------------------------------------------------------

async fn run_proxy(
    listener: TcpListener,
    target: SocketAddr,
    delay: Duration,
    bandwidth: Option<u64>,
) {
    while let Ok((inbound, _)) = listener.accept().await {
        let Ok(outbound) = TcpStream::connect(target).await else {
            continue;
        };
        let _ = inbound.set_nodelay(true);
        let _ = outbound.set_nodelay(true);
        let (from_client, to_client) = inbound.into_split();
        let (from_server, to_server) = outbound.into_split();
        tokio::spawn(pump(from_client, to_server, delay, bandwidth));
        tokio::spawn(pump(from_server, to_client, delay, bandwidth));
    }
}

/// Copies one direction: each piece leaves once the link has carried it (at `bandwidth`)
/// and `delay` has passed.
async fn pump(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    delay: Duration,
    bandwidth: Option<u64>,
) {
    let (tx, mut rx) = mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
    let writer = tokio::spawn(async move {
        let mut link_free = Instant::now();
        while let Some((sent, data)) = rx.recv().await {
            let start = sent.max(link_free);
            let busy = bandwidth.map_or(Duration::ZERO, |rate| {
                Duration::from_secs_f64(data.len() as f64 / rate as f64)
            });
            link_free = start + busy;
            let due = link_free + delay;
            if due > Instant::now() {
                tokio::time::sleep_until(due).await;
            }
            if to.write_all(&data).await.is_err() {
                break;
            }
        }
        let _ = to.shutdown().await;
    });
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx.send((Instant::now(), buf[..n].to_vec())).is_err() {
                    break;
                }
            }
        }
    }
    drop(tx);
    let _ = writer.await;
}

// ---- SSH ----------------------------------------------------------------------------

struct SshHandler {
    root: PathBuf,
    options: Options,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl russh::server::Handler for SshHandler {
    type Error = russh::Error;

    async fn auth_publickey(&mut self, _: &str, _: &PublicKey) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        match self.channels.remove(&id) {
            Some(channel) if name == "sftp" => {
                session.channel_success(id)?;
                let handler = FsHandler {
                    root: self.root.clone(),
                    options: self.options.clone(),
                    files: HashMap::new(),
                    dirs: HashMap::new(),
                    next: 0,
                };
                russh_sftp::server::run(channel.into_stream(), handler).await;
            }
            _ => session.channel_failure(id)?,
        }
        Ok(())
    }
}

// ---- SFTP ---------------------------------------------------------------------------

struct FsHandler {
    root: PathBuf,
    options: Options,
    files: HashMap<String, std::fs::File>,
    /// Entries not sent yet, per directory handle.
    dirs: HashMap<String, Vec<File>>,
    next: u64,
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".into(),
        language_tag: "en-US".into(),
    }
}

fn status_of(e: std::io::Error) -> StatusCode {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

/// `path` made absolute (from `/`) with `.` and `..` resolved.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

impl FsHandler {
    fn local(&self, path: &str) -> PathBuf {
        let mut local = self.root.clone();
        for part in normalize(path).split('/').filter(|p| !p.is_empty()) {
            local.push(part);
        }
        local
    }

    fn handle(&mut self) -> String {
        self.next += 1;
        format!("h{}", self.next)
    }

    fn attrs(&self, path: &Path) -> Result<FileAttributes, StatusCode> {
        let meta = std::fs::symlink_metadata(path).map_err(status_of)?;
        Ok(self.shown(FileAttributes::from(&meta)))
    }

    fn shown(&self, mut attrs: FileAttributes) -> FileAttributes {
        if self.options.hide_sizes && !attrs.is_dir() {
            attrs.size = Some(0);
        }
        attrs
    }
}

impl russh_sftp::server::Handler for FsHandler {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _: u32, _: HashMap<String, String>) -> Result<Version, Self::Error> {
        let mut version = Version::new();
        if self.options.limits {
            version
                .extensions
                .insert(russh_sftp::extensions::LIMITS.into(), "1".into());
        }
        Ok(version)
    }

    async fn extended(
        &mut self,
        id: u32,
        request: String,
        _: Vec<u8>,
    ) -> Result<Packet, Self::Error> {
        if !self.options.limits || request != russh_sftp::extensions::LIMITS {
            return Err(StatusCode::OpUnsupported);
        }
        let limits = LimitsExtension {
            max_packet_len: 256 * 1024,
            max_read_len: u64::from(self.options.max_read),
            max_write_len: 255 * 1024,
            max_open_handles: 0,
        };
        let data = russh_sftp::ser::to_bytes(&limits)
            .map_err(|_| StatusCode::Failure)?
            .to_vec();
        Ok(Packet::ExtendedReply(ExtendedReply { id, data }))
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let mut options = std::fs::OpenOptions::new();
        options
            .read(pflags.contains(OpenFlags::READ))
            .write(pflags.intersects(OpenFlags::WRITE | OpenFlags::APPEND))
            .truncate(pflags.contains(OpenFlags::TRUNCATE));
        if pflags.contains(OpenFlags::CREATE) {
            if pflags.contains(OpenFlags::EXCLUDE) {
                options.create_new(true);
            } else {
                options.create(true);
            }
        }
        let file = options.open(self.local(&filename)).map_err(status_of)?;
        let handle = self.handle();
        self.files.insert(handle.clone(), file);
        Ok(Handle { id, handle })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        let known = self.files.remove(&handle).is_some() | self.dirs.remove(&handle).is_some();
        if known {
            Ok(ok(id))
        } else {
            Err(StatusCode::Failure)
        }
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let len = len.min(self.options.max_read) as usize;
        let file = self.files.get_mut(&handle).ok_or(StatusCode::Failure)?;
        file.seek(SeekFrom::Start(offset)).map_err(status_of)?;
        let mut data = vec![0u8; len];
        let mut filled = 0;
        while filled < len {
            match file.read(&mut data[filled..]).map_err(status_of)? {
                0 => break,
                n => filled += n,
            }
        }
        if filled == 0 {
            return Err(StatusCode::Eof);
        }
        data.truncate(filled);
        Ok(Data { id, data })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let file = self.files.get_mut(&handle).ok_or(StatusCode::Failure)?;
        file.seek(SeekFrom::Start(offset)).map_err(status_of)?;
        file.write_all(&data).map_err(status_of)?;
        Ok(ok(id))
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        Ok(Attrs {
            id,
            attrs: self.attrs(&self.local(&path))?,
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let meta = std::fs::metadata(self.local(&path)).map_err(status_of)?;
        Ok(Attrs {
            id,
            attrs: self.shown(FileAttributes::from(&meta)),
        })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let file = self.files.get(&handle).ok_or(StatusCode::Failure)?;
        let meta = file.metadata().map_err(status_of)?;
        Ok(Attrs {
            id,
            attrs: self.shown(FileAttributes::from(&meta)),
        })
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        _: FileAttributes,
    ) -> Result<Status, Self::Error> {
        self.attrs(&self.local(&path))?;
        Ok(ok(id))
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        _: String,
        _: FileAttributes,
    ) -> Result<Status, Self::Error> {
        Ok(ok(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let dir = self.local(&path);
        let here = self.attrs(&dir)?;
        let mut entries = vec![File::new(".", here.clone()), File::new("..", here)];
        for item in std::fs::read_dir(&dir).map_err(status_of)? {
            let item = item.map_err(status_of)?;
            let name = item.file_name().to_string_lossy().into_owned();
            entries.push(File::new(name, self.attrs(&item.path())?));
        }
        let handle = self.handle();
        self.dirs.insert(handle.clone(), entries);
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let entries = self.dirs.get_mut(&handle).ok_or(StatusCode::Failure)?;
        if entries.is_empty() {
            return Err(StatusCode::Eof);
        }
        // Like OpenSSH: at most 100 entries per reply.
        let take = entries.len().min(100);
        let files = entries.drain(..take).collect();
        Ok(Name { id, files })
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        std::fs::remove_file(self.local(&filename)).map_err(status_of)?;
        Ok(ok(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _: FileAttributes,
    ) -> Result<Status, Self::Error> {
        std::fs::create_dir(self.local(&path)).map_err(status_of)?;
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        std::fs::remove_dir(self.local(&path)).map_err(status_of)?;
        Ok(ok(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![File::dummy(normalize(&path))],
        })
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let to = self.local(&newpath);
        if to.exists() {
            return Err(StatusCode::Failure);
        }
        std::fs::rename(self.local(&oldpath), to).map_err(status_of)?;
        Ok(ok(id))
    }
}
