//! Interactive SSH sessions, and local terminals driven the same way.
//!
//! A session is a long-lived task that owns one terminal connection: it connects
//! (through jump hosts if needed), authenticates, runs a PTY shell, streams output to an
//! [`EventSink`] and survives disconnects so it can be reconnected in place. A local
//! session ([`SessionManager::open_local`]) runs a program on this computer instead.
//!
//! The API is UI-agnostic: a GUI, TUI or CLI supplies an [`EventSink`] and answers
//! [`Prompt`]s (host key confirmation, passwords, passphrases, 2FA codes) through
//! [`SessionManager::answer`].

mod auth;
mod connect;
pub(crate) mod handler;
mod local;
mod shell;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use russh::client::Handle;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};
use zeroize::Zeroizing;

use self::handler::ClientHandler;
use crate::error::{Error, Result};
use crate::exec::{self, ExecOptions, ExecOutput};
use crate::forward::ForwardInfo;
use crate::i18n;
use crate::known_hosts::{HostKeyStatus, KnownHosts};
use crate::local::LocalCommand;
use crate::model::{ForwardSpec, PtySize, Server};
use crate::secrets::Secrets;
use crate::sftp::Sftp;
use crate::store::ServerStore;

pub type SessionId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    Connecting,
    Connected,
    /// The connection ended; the session can be reconnected.
    Disconnected,
    /// The session was closed and its task has finished.
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// A question the user must answer before the connection can proceed.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Prompt {
    #[serde(rename_all = "camelCase")]
    HostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        check: HostKeyStatus,
    },
    /// The host has no user name: which one to log in as (PuTTY's "login as:").
    #[serde(rename_all = "camelCase")]
    User {
        host: String,
        /// The name typed last for it in this session, else this computer's user name, which
        /// OpenSSH would take.
        suggestion: String,
        /// The saved server it is, which the page may remember the name for once it worked.
        #[serde(skip_serializing_if = "Option::is_none")]
        server_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Password {
        user: String,
        host: String,
        can_remember: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Passphrase {
        key_path: String,
        can_remember: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    KeyboardInteractive {
        host: String,
        name: String,
        instructions: String,
        prompts: Vec<KbdPrompt>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbdPrompt {
    pub text: String,
    pub echo: bool,
}

/// The user's answer to a [`Prompt`].
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PromptReply {
    HostKey {
        accept: bool,
        #[serde(default)]
        remember: bool,
    },
    Secret {
        value: String,
        #[serde(default)]
        remember: bool,
    },
    Answers {
        values: Vec<String>,
    },
    User {
        name: String,
    },
    Cancel,
}

/// Events emitted by a session (terminal output is delivered separately as raw bytes).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SessionEvent {
    Status {
        status: SessionStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        /// For `Disconnected`: the connection failed or was lost, as opposed to ending
        /// normally (the shell exited, the user disconnected or cancelled the login).
        failed: bool,
    },
    Log {
        level: LogLevel,
        message: String,
    },
    Prompt {
        id: u64,
        prompt: Prompt,
    },
    PromptClosed {
        id: u64,
    },
    Forwards {
        forwards: Vec<ForwardInfo>,
    },
}

/// Receives everything a session produces. Implementations must be cheap and
/// non-blocking (they are called from the session task).
pub trait EventSink: Send + Sync + 'static {
    fn event(&self, event: SessionEvent);
    /// Raw terminal output (already coalesced into reasonably sized chunks).
    fn output(&self, data: Vec<u8>);
}

pub(crate) enum Command {
    Write(Vec<u8>),
    Resize(PtySize),
    Reconnect,
    /// Drops the connection (or aborts connecting) but keeps the session reconnectable.
    Disconnect,
    Close,
    AddForward(ForwardSpec, oneshot::Sender<Result<()>>),
    RemoveForward(u64),
}

pub(crate) struct Shared {
    pub store: Arc<ServerStore>,
    pub secrets: Arc<Secrets>,
    pub known_hosts: Arc<KnownHosts>,
    sessions: Mutex<HashMap<SessionId, mpsc::UnboundedSender<Command>>>,
    prompts: Mutex<HashMap<u64, oneshot::Sender<PromptReply>>>,
    /// Connections of connected sessions, for extra channels such as SFTP.
    live: Mutex<HashMap<SessionId, Live>>,
    /// Counts changes of `live`, so others can wait for a session to connect.
    live_changes: watch::Sender<u64>,
    next_session: AtomicU64,
    next_prompt: AtomicU64,
}

/// Owns all running sessions.
#[derive(Clone)]
pub struct SessionManager {
    shared: Arc<Shared>,
}

impl SessionManager {
    pub fn new(
        store: Arc<ServerStore>,
        secrets: Arc<Secrets>,
        known_hosts: Arc<KnownHosts>,
    ) -> Self {
        SessionManager {
            shared: Arc::new(Shared {
                store,
                secrets,
                known_hosts,
                sessions: Mutex::new(HashMap::new()),
                prompts: Mutex::new(HashMap::new()),
                live: Mutex::new(HashMap::new()),
                live_changes: watch::Sender::new(0),
                next_session: AtomicU64::new(1),
                next_prompt: AtomicU64::new(1),
            }),
        }
    }

    /// Starts a session and returns immediately; progress is reported to `sink`.
    /// Must be called from within a Tokio runtime.
    pub fn open(&self, server: Server, size: PtySize, sink: Arc<dyn EventSink>) -> SessionId {
        let (ctx, rx) = self.register(sink);
        let id = ctx.id;
        tokio::spawn(shell::run(ctx, server, size, rx));
        id
    }

    /// Starts a local terminal running `command` (see [`crate::local`]) and returns
    /// immediately. It is driven like an SSH session: `reconnect` starts the program again,
    /// `disconnect` stops it. Must be called from within a Tokio runtime.
    pub fn open_local(
        &self,
        command: LocalCommand,
        size: PtySize,
        sink: Arc<dyn EventSink>,
    ) -> SessionId {
        let (ctx, rx) = self.register(sink);
        let id = ctx.id;
        tokio::spawn(local::run(ctx, command, size, rx));
        id
    }

    fn register(
        &self,
        sink: Arc<dyn EventSink>,
    ) -> (Arc<SessionCtx>, mpsc::UnboundedReceiver<Command>) {
        let id = self.shared.next_session.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        lock(&self.shared.sessions).insert(id, tx);
        let ctx = Arc::new(SessionCtx {
            id,
            sink,
            shared: Arc::clone(&self.shared),
            active_prompts: AtomicUsize::new(0),
            prompted: AtomicBool::new(false),
            cache: Mutex::new(CredCache::default()),
        });
        (ctx, rx)
    }

    fn send(&self, id: SessionId, cmd: Command) -> Result<()> {
        let sessions = lock(&self.shared.sessions);
        let tx = sessions
            .get(&id)
            .ok_or_else(|| Error::NotFound(i18n::session_not_found(id)))?;
        tx.send(cmd)
            .map_err(|_| Error::NotFound(i18n::session_not_found(id)))
    }

    pub fn write(&self, id: SessionId, data: Vec<u8>) -> Result<()> {
        self.send(id, Command::Write(data))
    }

    pub fn resize(&self, id: SessionId, size: PtySize) -> Result<()> {
        self.send(id, Command::Resize(size))
    }

    /// Reconnects a disconnected session (or restarts a live one) in place.
    pub fn reconnect(&self, id: SessionId) -> Result<()> {
        self.send(id, Command::Reconnect)
    }

    /// Drops the connection but keeps the session so it can be reconnected.
    pub fn disconnect(&self, id: SessionId) -> Result<()> {
        self.send(id, Command::Disconnect)
    }

    /// Closes a session; its task emits a final `Closed` status.
    pub fn close(&self, id: SessionId) {
        let _ = self.send(id, Command::Close);
    }

    pub fn close_all(&self) {
        let ids: Vec<SessionId> = lock(&self.shared.sessions).keys().copied().collect();
        for id in ids {
            self.close(id);
        }
    }

    pub fn session_count(&self) -> usize {
        lock(&self.shared.sessions).len()
    }

    /// Delivers the user's answer to a pending prompt.
    pub fn answer(&self, prompt_id: u64, reply: PromptReply) -> Result<()> {
        let tx = lock(&self.shared.prompts)
            .remove(&prompt_id)
            .ok_or_else(|| Error::NotFound(i18n::prompt_not_found()))?;
        let _ = tx.send(reply);
        Ok(())
    }

    /// Starts a port forward on a connected session.
    pub async fn add_forward(&self, id: SessionId, spec: ForwardSpec) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send(id, Command::AddForward(spec, tx))?;
        rx.await
            .map_err(|_| Error::Disconnected(i18n::session_ended_early()))?
    }

    pub fn remove_forward(&self, id: SessionId, forward_id: u64) -> Result<()> {
        self.send(id, Command::RemoveForward(forward_id))
    }

    /// The SFTP client of a connected session, opened on first use and kept until the
    /// connection ends (a reconnected session opens a new one).
    pub async fn sftp(&self, id: SessionId) -> Result<Arc<Sftp>> {
        let (handle, cell) = {
            let live = lock(&self.shared.live);
            let entry = live
                .get(&id)
                .ok_or_else(|| Error::Disconnected(i18n::not_connected()))?;
            (Arc::clone(&entry.handle), Arc::clone(&entry.sftp))
        };
        cell.get_or_try_init(|| async { Sftp::open(&handle).await.map(Arc::new) })
            .await
            .cloned()
    }

    /// Runs `command` on a connected session's connection, on a channel of its own and
    /// without a terminal (see [`crate::exec`]).
    pub async fn exec(
        &self,
        id: SessionId,
        command: &str,
        options: ExecOptions,
    ) -> Result<ExecOutput> {
        let handle = {
            let live = lock(&self.shared.live);
            let entry = live
                .get(&id)
                .ok_or_else(|| Error::Disconnected(i18n::not_connected()))?;
            Arc::clone(&entry.handle)
        };
        exec::run(&handle, command, options).await
    }

    /// A connected session of the saved server `server_id` (the one connected first).
    pub fn connected(&self, server_id: &str) -> Option<SessionId> {
        lock(&self.shared.live)
            .iter()
            .filter(|(_, live)| !server_id.is_empty() && live.server_id == server_id)
            .map(|(id, _)| *id)
            .min()
    }

    /// Changes whenever a session connects or its connection ends.
    pub fn live_changes(&self) -> watch::Receiver<u64> {
        self.shared.live_changes.subscribe()
    }
}

/// A connected session's SSH connection.
pub(crate) struct Live {
    /// The saved server it is (empty for quick connections).
    server_id: String,
    handle: Arc<Handle<ClientHandler>>,
    sftp: Arc<tokio::sync::OnceCell<Arc<Sftp>>>,
}

impl Shared {
    pub(crate) fn set_live(
        &self,
        id: SessionId,
        server_id: &str,
        handle: Arc<Handle<ClientHandler>>,
    ) {
        lock(&self.live).insert(
            id,
            Live {
                server_id: server_id.to_string(),
                handle,
                sftp: Arc::default(),
            },
        );
        self.live_changes.send_modify(|n| *n += 1);
    }

    pub(crate) fn clear_live(&self, id: SessionId) {
        lock(&self.live).remove(&id);
        self.live_changes.send_modify(|n| *n += 1);
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Secrets typed by the user during this session, reused on reconnect so the user
/// is not asked again (kept in memory only, zeroized on drop).
#[derive(Default)]
pub(crate) struct CredCache {
    pub passwords: HashMap<String, Zeroizing<String>>,
    pub passphrases: HashMap<PathBuf, Zeroizing<String>>,
    /// User names typed for hosts without one (`Prompt::User`), by `host:port`.
    pub users: HashMap<String, TypedUser>,
}

/// A user name typed for a host without one.
pub(crate) struct TypedUser {
    pub name: String,
    /// Logging in with it worked: reconnects use it without asking.
    pub worked: bool,
}

/// Per-session state shared by the connection, handler and auth code.
pub(crate) struct SessionCtx {
    pub id: SessionId,
    pub sink: Arc<dyn EventSink>,
    pub shared: Arc<Shared>,
    active_prompts: AtomicUsize,
    /// Set whenever a prompt was shown; lets timeouts ignore time spent waiting for the user.
    prompted: AtomicBool,
    pub cache: Mutex<CredCache>,
}

impl SessionCtx {
    pub fn event(&self, event: SessionEvent) {
        self.sink.event(event);
    }

    pub fn status(&self, status: SessionStatus) {
        self.event(SessionEvent::Status {
            status,
            message: None,
            failed: false,
        });
    }

    /// Reports that the connection ended; the session stays reconnectable.
    pub fn disconnected(&self, message: String, failed: bool) {
        self.event(SessionEvent::Status {
            status: SessionStatus::Disconnected,
            message: Some(message),
            failed,
        });
    }

    pub fn log(&self, level: LogLevel, message: impl Into<String>) {
        let message = message.into();
        match level {
            LogLevel::Info => log::info!("session {}: {message}", self.id),
            LogLevel::Warn => log::warn!("session {}: {message}", self.id),
            LogLevel::Error => log::error!("session {}: {message}", self.id),
        }
        self.event(SessionEvent::Log { level, message });
    }

    /// Whether a prompt was active since the last call (clears the flag).
    pub fn take_prompted(&self) -> bool {
        self.prompted.swap(false, Ordering::Relaxed)
            || self.active_prompts.load(Ordering::Relaxed) > 0
    }

    /// Shows `prompt` to the user and waits for the answer. `None` means cancelled.
    pub async fn ask(&self, prompt: Prompt) -> Option<PromptReply> {
        let (tx, rx) = oneshot::channel();
        let id = self.shared.next_prompt.fetch_add(1, Ordering::Relaxed);
        lock(&self.shared.prompts).insert(id, tx);
        let _guard = PromptGuard { ctx: self, id };
        self.active_prompts.fetch_add(1, Ordering::Relaxed);
        self.prompted.store(true, Ordering::Relaxed);
        self.event(SessionEvent::Prompt { id, prompt });
        match rx.await {
            Ok(PromptReply::Cancel) | Err(_) => None,
            Ok(reply) => Some(reply),
        }
    }
}

/// Unregisters a prompt and tells the UI to hide it, whether it was answered or abandoned.
struct PromptGuard<'a> {
    ctx: &'a SessionCtx,
    id: u64,
}

impl Drop for PromptGuard<'_> {
    fn drop(&mut self) {
        lock(&self.ctx.shared.prompts).remove(&self.id);
        self.ctx.active_prompts.fetch_sub(1, Ordering::Relaxed);
        self.ctx.prompted.store(true, Ordering::Relaxed);
        self.ctx.event(SessionEvent::PromptClosed { id: self.id });
    }
}
