//! AI agents: NexSSH as an MCP server (Model Context Protocol), so agents such as Claude
//! Code, Codex or Cursor can work on the servers the user shares with them.
//!
//! * Transport: Streamable HTTP on `127.0.0.1:<port>/mcp` ([`http`]) with a bearer token kept
//!   in the OS keychain. `NexSSH mcp` bridges an agent's stdio to it ([`bridge`]), so an
//!   agent's configuration needs no token at all (the argument has no dashes: shells and
//!   agents' command lines then cannot take it for one of their own options).
//! * Protocol: JSON-RPC as MCP's revisions from 2024-11-05 to 2026-07-28 speak it ([`rpc`]).
//! * Tools ([`tools`]): the shared servers, commands, files, folders, uploads and downloads,
//!   and what a server's terminal tab shows (the page reads it from the terminal and answers,
//!   see [`Agents::read_screen`]).
//!   They work on the user's own sessions: a command runs on a channel of a connected tab's
//!   connection (no second login), and for a server without one the page opens a tab, where
//!   the user answers a password or 2FA prompt like for any other.
//! * Each server says what agents may do ([`AgentAccess`]): nothing (the default), ask first,
//!   or anything. Questions go to the page as requests; what agents did is kept as activity.
//!
//! The page sees all of it as one status: [`Agents::status`], sent again on every change (the
//! `agents` event), so a page that loads late misses nothing.

mod bridge;
mod http;
mod rpc;
mod tools;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nexssh_core::AgentAccess;
use nexssh_core::secrets::Secrets;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;

use crate::commands::{AppState, CmdResult, blocking};

pub use bridge::run as run_bridge;

/// Where agents connect unless the settings say otherwise.
pub const DEFAULT_PORT: u16 = 7422;
/// Keychain account of the bearer token.
const TOKEN_ACCOUNT: &str = "agents-token";
/// Kept in the activity, newest first.
const MAX_ACTIVITY: usize = 60;
/// Agents shown as recently connected.
const RECENT_AGENTS: Duration = Duration::from_secs(60 * 60);
/// MCP sessions remembered (the least recently used one is forgotten).
const MAX_CLIENTS: usize = 64;
/// How long the page may take to read a terminal.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The page's settings this module reads (`settings.json` belongs to the page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub enabled: bool,
    pub port: u16,
}

impl Config {
    pub fn from_settings(settings: &Value) -> Config {
        Config {
            enabled: settings
                .get("agentsEnabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            port: settings
                .get("agentsPort")
                .and_then(Value::as_u64)
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0)
                .unwrap_or(DEFAULT_PORT),
        }
    }
}

/// Who is calling: an MCP client, by the name it gives.
#[derive(Debug, Clone)]
pub(crate) struct Caller {
    /// Its MCP session, or its name when it uses none: what "don't ask again" is kept for.
    pub key: String,
    /// As shown to the user ("Claude Code").
    pub name: String,
}

/// A question for the user: may an agent do this?
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: u64,
    pub agent: String,
    pub server_id: String,
    pub server_name: String,
    /// `run_command`, `write_file` or `upload`.
    pub tool: String,
    /// The command, or the path.
    pub detail: String,
    /// What goes with it: the input of a command, the beginning of a file's new content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// An agent needs a session of a server that has none: the page opens a tab.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Open {
    pub id: u64,
    pub agent: String,
    pub server_id: String,
}

/// What a terminal tab shows, as the page reads it from the terminal.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Screen {
    pub title: String,
    /// `connecting`, `connected` or `disconnected`.
    pub status: String,
    /// Which of the server's tabs, and how many it has.
    pub tab: u32,
    pub tabs: u32,
    pub cols: u32,
    pub rows: u32,
    /// From 1, on the screen.
    pub cursor_row: u32,
    pub cursor_col: u32,
    /// A full-screen program (vim, htop, less) has the screen.
    pub alternate: bool,
    /// Lines in the scrollback above the screen.
    pub above: u32,
    /// The first row given (from 1), and the last one with something on it: where the output
    /// ends (0 when nothing has been printed).
    pub from: u32,
    pub total: u32,
    /// The lines, without colours; a long line the terminal wrapped is one line again.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ActivityStatus {
    /// Waiting for the user (a question, or a tab connecting).
    Waiting,
    Running,
    Done,
    Failed,
    Denied,
}

/// Something an agent did, or is doing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub id: u64,
    /// Unix time in milliseconds.
    pub at: u64,
    pub agent: String,
    pub server_id: String,
    pub server_name: String,
    pub tool: String,
    pub detail: String,
    pub status: ActivityStatus,
    /// The tab's session it ran on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<u64>,
    /// A command's exit code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Seen {
    pub name: String,
    /// Unix time in milliseconds.
    pub last_seen: u64,
}

/// Everything the page shows about agents.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// This program, for agents' configurations (`NexSSH mcp`).
    pub exe: String,
    /// The token outlives this run (it is in the keychain), so the bridge can use it.
    pub token_kept: bool,
    pub agents: Vec<Seen>,
    pub requests: Vec<Request>,
    pub opens: Vec<Open>,
    pub activity: Vec<Activity>,
}

struct Client {
    name: String,
    last_seen: u64,
}

#[derive(Default)]
struct Inner {
    config: Option<Config>,
    /// The listening task.
    server: Option<JoinHandle<()>>,
    /// The port it listens on, once it does.
    listening: Option<u16>,
    error: Option<String>,
    token: Option<String>,
    token_kept: bool,
    /// MCP sessions (protocol revisions before 2026-07-28), by id.
    clients: HashMap<String, Client>,
    /// Agents by name, with when they were last heard from.
    seen: HashMap<String, u64>,
    requests: Vec<(Request, oneshot::Sender<Answer>)>,
    opens: Vec<(Open, oneshot::Sender<String>)>,
    /// Terminals the page is asked to read; it answers with the text, or why not.
    reads: HashMap<u64, oneshot::Sender<Result<Screen, String>>>,
    activity: VecDeque<Activity>,
    /// "Don't ask again": caller key and server id.
    trusted: HashSet<(String, String)>,
    next_id: u64,
}

impl Inner {
    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
}

/// The user's answer to a [`Request`].
#[derive(Debug, Clone, Copy)]
struct Answer {
    allow: bool,
    /// Don't ask this agent again about this server until NexSSH quits.
    remember: bool,
}

#[derive(Default)]
pub struct Agents {
    inner: Mutex<Inner>,
    app: OnceLock<AppHandle>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A new bearer token: 256 random bits as hex.
fn new_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system's random number generator works");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares in constant time, so the token cannot be guessed byte by byte from timings.
fn same_secret(given: &str, expected: &str) -> bool {
    let (a, b) = (given.as_bytes(), expected.as_bytes());
    let mut diff = a.len() ^ b.len();
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= usize::from(x ^ y);
    }
    diff == 0 && !expected.is_empty()
}

impl Agents {
    /// Starts serving agents if the settings say so.
    pub fn init(&self, app: &AppHandle, settings: &Value) {
        let _ = self.app.set(app.clone());
        self.apply(settings);
    }

    /// Follows the settings: starts, stops, or moves to another port.
    pub fn apply(&self, settings: &Value) {
        let config = Config::from_settings(settings);
        let mut inner = lock(&self.inner);
        if inner.config == Some(config) {
            return;
        }
        let previous = inner.config.replace(config);
        let restart = previous.is_none_or(|p| p.port != config.port || !p.enabled);
        if !config.enabled {
            Self::stop(&mut inner);
        } else if restart || inner.server.is_none() {
            Self::stop(&mut inner);
            if let Some(app) = self.app.get() {
                inner.server = Some(tauri::async_runtime::spawn(serve(app.clone(), config.port)));
            }
        }
        drop(inner);
        self.changed();
    }

    fn stop(inner: &mut Inner) {
        if let Some(server) = inner.server.take() {
            server.abort();
        }
        inner.listening = None;
        inner.error = None;
        // Nobody can answer those any more: requests are denied, tabs no longer awaited.
        inner.requests.clear();
        inner.opens.clear();
        inner.reads.clear();
    }

    pub fn status(&self) -> Status {
        let inner = lock(&self.inner);
        let config = inner.config.unwrap_or(Config {
            enabled: false,
            port: DEFAULT_PORT,
        });
        let since = now_ms().saturating_sub(RECENT_AGENTS.as_millis() as u64);
        let mut agents: Vec<Seen> = inner
            .seen
            .iter()
            .filter(|(_, at)| **at >= since)
            .map(|(name, at)| Seen {
                name: name.clone(),
                last_seen: *at,
            })
            .collect();
        agents.sort_by_key(|a| std::cmp::Reverse(a.last_seen));
        Status {
            enabled: config.enabled,
            running: inner.listening.is_some(),
            port: config.port,
            url: format!("http://127.0.0.1:{}/mcp", config.port),
            error: inner.error.clone(),
            exe: std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            token_kept: inner.token_kept,
            agents,
            requests: inner.requests.iter().map(|(r, _)| r.clone()).collect(),
            opens: inner.opens.iter().map(|(o, _)| o.clone()).collect(),
            activity: inner.activity.iter().cloned().collect(),
        }
    }

    /// Tells the page what changed.
    fn changed(&self) {
        if let Some(app) = self.app.get() {
            let _ = app.emit("agents", self.status());
        }
    }

    // ---- token ------------------------------------------------------------------------

    /// The bearer token: from the keychain, or a new one (kept there when it can be).
    /// Blocking: the keychain may be slow.
    fn token_blocking(&self, secrets: &Secrets) -> String {
        if let Some(token) = lock(&self.inner).token.clone() {
            return token;
        }
        let (token, kept) = match secrets.get(TOKEN_ACCOUNT) {
            Ok(Some(token)) if !token.is_empty() => (token.to_string(), true),
            _ => {
                let token = new_token();
                let kept = secrets.set(TOKEN_ACCOUNT, &token).is_ok() && secrets.is_available();
                (token, kept)
            }
        };
        let mut inner = lock(&self.inner);
        // Another caller may have been quicker; one token for everyone.
        let token = inner.token.get_or_insert(token).clone();
        inner.token_kept = kept;
        token
    }

    /// Replaces the token: agents holding the old one are refused from now on.
    fn renew_token_blocking(&self, secrets: &Secrets) -> String {
        let token = new_token();
        let kept = secrets.set(TOKEN_ACCOUNT, &token).is_ok() && secrets.is_available();
        let mut inner = lock(&self.inner);
        inner.token = Some(token.clone());
        inner.token_kept = kept;
        inner.clients.clear();
        token
    }

    fn authorized(&self, bearer: Option<&str>) -> bool {
        let inner = lock(&self.inner);
        match (bearer, inner.token.as_deref()) {
            (Some(given), Some(expected)) => same_secret(given, expected),
            _ => false,
        }
    }

    // ---- callers ------------------------------------------------------------------------

    /// A new MCP session for a client that introduced itself; returns its id.
    fn add_client(&self, name: &str) -> String {
        let id = new_token()[..32].to_string();
        let mut inner = lock(&self.inner);
        if inner.clients.len() >= MAX_CLIENTS
            && let Some(oldest) = inner
                .clients
                .iter()
                .min_by_key(|(_, c)| c.last_seen)
                .map(|(id, _)| id.clone())
        {
            inner.clients.remove(&oldest);
        }
        let now = now_ms();
        inner.clients.insert(
            id.clone(),
            Client {
                name: name.to_string(),
                last_seen: now,
            },
        );
        inner.seen.insert(name.to_string(), now);
        drop(inner);
        self.changed();
        id
    }

    /// The caller of an MCP session; `None` when the session is unknown (NexSSH restarted, or
    /// the token changed): the client then starts a new one.
    fn client(&self, session: &str) -> Option<Caller> {
        let mut inner = lock(&self.inner);
        let now = now_ms();
        let client = inner.clients.get_mut(session)?;
        client.last_seen = now;
        let name = client.name.clone();
        let fresh = inner
            .seen
            .insert(name.clone(), now)
            .is_none_or(|at| now - at > 60_000);
        drop(inner);
        if fresh {
            self.changed();
        }
        Some(Caller {
            key: session.to_string(),
            name,
        })
    }

    /// A caller without a session (MCP 2026-07-28 has none), by the name it gives.
    fn stateless(&self, name: &str) -> Caller {
        let now = now_ms();
        let fresh = lock(&self.inner)
            .seen
            .insert(name.to_string(), now)
            .is_none_or(|at| now - at > 60_000);
        if fresh {
            self.changed();
        }
        Caller {
            key: format!("agent:{name}"),
            name: name.to_string(),
        }
    }

    fn end_client(&self, session: &str) {
        lock(&self.inner).clients.remove(session);
    }

    // ---- questions --------------------------------------------------------------------

    /// Whether the user lets `caller` do this on `server`: asked unless the server allows
    /// anything, or the user said not to ask this agent again. `Err` says why not.
    async fn ask(
        &self,
        caller: &Caller,
        server: &nexssh_core::Server,
        tool: &str,
        detail: &str,
        preview: Option<String>,
    ) -> Result<(), String> {
        match server.agents {
            AgentAccess::Allow => return Ok(()),
            AgentAccess::Off => return Err(tools::not_shared(&server.name)),
            AgentAccess::Ask => {}
        }
        let (tx, rx) = oneshot::channel();
        let _withdraw = {
            let mut inner = lock(&self.inner);
            if inner
                .trusted
                .contains(&(caller.key.clone(), server.id.clone()))
            {
                return Ok(());
            }
            let id = inner.next_id();
            let request = Request {
                id,
                agent: caller.name.clone(),
                server_id: server.id.clone(),
                server_name: server.name.clone(),
                tool: tool.to_string(),
                detail: detail.to_string(),
                preview,
            };
            inner.requests.push((request, tx));
            Withdraw {
                agents: self,
                id,
                kind: Waiting::Request,
            }
        };
        self.changed();
        self.attention();
        let answer = match tokio::time::timeout(tools::ANSWER_TIMEOUT, rx).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => return Err(tools::NOT_ANSWERED.to_string()),
            Err(_) => return Err(tools::NOT_ANSWERED_IN_TIME.to_string()),
        };
        if answer.allow && answer.remember {
            lock(&self.inner)
                .trusted
                .insert((caller.key.clone(), server.id.clone()));
        }
        if answer.allow {
            Ok(())
        } else {
            Err(tools::DECLINED.to_string())
        }
    }

    /// The page's answer to a request.
    pub fn answer(&self, id: u64, allow: bool, remember: bool) {
        let mut inner = lock(&self.inner);
        if let Some(i) = inner.requests.iter().position(|(r, _)| r.id == id) {
            let (_, tx) = inner.requests.remove(i);
            let _ = tx.send(Answer { allow, remember });
        }
        drop(inner);
        self.changed();
    }

    /// The server's access changed (or it was deleted): "don't ask again" no longer applies.
    pub fn forget_server(&self, server_id: &str) {
        lock(&self.inner)
            .trusted
            .retain(|(_, server)| server != server_id);
    }

    /// Brings the window back from the tray, and flashes the taskbar button (Windows) or
    /// bounces the dock icon (macOS) while NexSSH waits for the user.
    fn attention(&self) {
        if let Some(app) = self.app.get() {
            crate::tray::attention(app);
        }
    }

    // ---- tabs ---------------------------------------------------------------------------

    /// Asks the page to open a tab of the server, until the guard is dropped; the receiver gets
    /// why it failed, if it did.
    fn open_tab(
        &self,
        caller: &Caller,
        server_id: &str,
    ) -> (Withdraw<'_>, oneshot::Receiver<String>) {
        let (tx, rx) = oneshot::channel();
        let mut inner = lock(&self.inner);
        let id = inner.next_id();
        inner.opens.push((
            Open {
                id,
                agent: caller.name.clone(),
                server_id: server_id.to_string(),
            },
            tx,
        ));
        drop(inner);
        self.changed();
        let guard = Withdraw {
            agents: self,
            id,
            kind: Waiting::Open,
        };
        (guard, rx)
    }

    /// The page could not connect the tab it opened for an agent.
    pub fn open_failed(&self, id: u64, message: String) {
        let mut inner = lock(&self.inner);
        if let Some(i) = inner.opens.iter().position(|(o, _)| o.id == id) {
            let (_, tx) = inner.opens.remove(i);
            let _ = tx.send(message);
        }
        drop(inner);
        self.changed();
    }

    // ---- terminals ------------------------------------------------------------------

    /// Has the page read a terminal tab of the server: tab number `tab` (from 1), else the
    /// active one, else the last; the screen, or the last `lines` lines. `Err` holds the page's
    /// reason (`noTab`, `noSuchTab`, `notReady`) or a message.
    pub(crate) async fn read_screen(
        &self,
        server_id: &str,
        tab: Option<u64>,
        lines: Option<u64>,
    ) -> Result<Screen, String> {
        let Some(app) = self.app.get() else {
            return Err(tools::PAGE_SILENT.to_string());
        };
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut inner = lock(&self.inner);
            let id = inner.next_id();
            inner.reads.insert(id, tx);
            id
        };
        let request = json!({ "id": id, "serverId": server_id, "tab": tab, "lines": lines });
        let _ = app.emit("agents-read", request);
        let answer = tokio::time::timeout(READ_TIMEOUT, rx).await;
        lock(&self.inner).reads.remove(&id);
        match answer {
            Ok(Ok(result)) => result,
            _ => Err(tools::PAGE_SILENT.to_string()),
        }
    }

    /// The page's answer to [`Agents::read_screen`].
    pub fn screen_read(&self, id: u64, screen: Option<Screen>, error: Option<String>) {
        if let Some(tx) = lock(&self.inner).reads.remove(&id) {
            let _ = tx.send(screen.ok_or_else(|| error.unwrap_or_default()));
        }
    }

    // ---- activity ---------------------------------------------------------------------

    fn begin(
        &self,
        caller: &Caller,
        server: &nexssh_core::Server,
        tool: &str,
        detail: &str,
    ) -> u64 {
        let mut inner = lock(&self.inner);
        let id = inner.next_id();
        inner.activity.push_front(Activity {
            id,
            at: now_ms(),
            agent: caller.name.clone(),
            server_id: server.id.clone(),
            server_name: server.name.clone(),
            tool: tool.to_string(),
            detail: tools::clip(detail, 500),
            status: ActivityStatus::Waiting,
            session_id: None,
            exit_code: None,
            duration_ms: None,
            error: None,
        });
        inner.activity.truncate(MAX_ACTIVITY);
        drop(inner);
        self.changed();
        id
    }

    fn update(&self, id: u64, change: impl FnOnce(&mut Activity)) {
        let mut inner = lock(&self.inner);
        if let Some(item) = inner.activity.iter_mut().find(|a| a.id == id) {
            change(item);
            if matches!(
                item.status,
                ActivityStatus::Done | ActivityStatus::Failed | ActivityStatus::Denied
            ) {
                item.duration_ms = Some(now_ms().saturating_sub(item.at));
            }
        }
        drop(inner);
        self.changed();
    }

    /// Empties the activity list (requests in progress stay).
    pub fn clear_activity(&self) {
        lock(&self.inner)
            .activity
            .retain(|a| matches!(a.status, ActivityStatus::Waiting | ActivityStatus::Running));
        self.changed();
    }
}

#[derive(Clone, Copy)]
enum Waiting {
    Request,
    Open,
}

/// Takes a request, or a tab being awaited, off the page's list once nobody waits for it any
/// more: it was answered, it timed out, or the agent stopped waiting (its call was dropped).
struct Withdraw<'a> {
    agents: &'a Agents,
    id: u64,
    kind: Waiting,
}

impl Drop for Withdraw<'_> {
    fn drop(&mut self) {
        let mut inner = lock(&self.agents.inner);
        let before = inner.requests.len() + inner.opens.len();
        match self.kind {
            Waiting::Request => inner.requests.retain(|(r, _)| r.id != self.id),
            Waiting::Open => inner.opens.retain(|(o, _)| o.id != self.id),
        }
        let gone = before != inner.requests.len() + inner.opens.len();
        drop(inner);
        if gone {
            self.agents.changed();
        }
    }
}

/// Listens on the port until stopped (the task is aborted).
async fn serve(app: AppHandle, port: u16) {
    let agents = app.state::<Agents>();
    let secrets = Arc::clone(&app.state::<AppState>().core.secrets);
    let token_ready = {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            app.state::<Agents>().token_blocking(&secrets);
        })
        .await
    };
    if let Err(e) = token_ready {
        log::error!("agents: no token: {e}");
    }
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(e) => {
            log::warn!("agents: cannot listen on port {port}: {e}");
            lock(&agents.inner).error = Some(if e.kind() == std::io::ErrorKind::AddrInUse {
                nexssh_core::i18n::agents_port_busy(port)
            } else {
                e.to_string()
            });
            agents.changed();
            return;
        }
    };
    log::info!("agents: MCP on http://127.0.0.1:{port}/mcp");
    {
        let mut inner = lock(&agents.inner);
        inner.listening = Some(port);
        inner.error = None;
    }
    agents.changed();
    http::serve(listener, app.clone()).await;
}

// ---- IPC ------------------------------------------------------------------------------

#[tauri::command]
pub fn agents_status(agents: tauri::State<'_, Agents>) -> Status {
    agents.status()
}

/// The bearer token, for agents that connect over HTTP.
#[tauri::command]
pub async fn agents_token(app: AppHandle) -> CmdResult<String> {
    blocking(move || {
        let secrets = Arc::clone(&app.state::<AppState>().core.secrets);
        let token = app.state::<Agents>().token_blocking(&secrets);
        app.state::<Agents>().changed();
        token
    })
    .await
}

/// A new bearer token; the old one stops working.
#[tauri::command]
pub async fn agents_new_token(app: AppHandle) -> CmdResult<String> {
    blocking(move || {
        let secrets = Arc::clone(&app.state::<AppState>().core.secrets);
        let token = app.state::<Agents>().renew_token_blocking(&secrets);
        app.state::<Agents>().changed();
        token
    })
    .await
}

#[tauri::command]
pub fn agents_answer(agents: tauri::State<'_, Agents>, id: u64, allow: bool, remember: bool) {
    agents.answer(id, allow, remember);
}

/// The tab opened for an agent did not connect.
#[tauri::command]
pub fn agents_open_failed(agents: tauri::State<'_, Agents>, id: u64, message: String) {
    agents.open_failed(id, message);
}

/// What a terminal tab shows, read by the page for an agent (`agents-read`).
#[tauri::command]
pub fn agents_screen(
    agents: tauri::State<'_, Agents>,
    id: u64,
    screen: Option<Screen>,
    error: Option<String>,
) {
    agents.screen_read(id, screen, error);
}

#[tauri::command]
pub fn agents_clear_activity(agents: tauri::State<'_, Agents>) {
    agents.clear_activity();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_its_settings() {
        let config = Config::from_settings(&json!({}));
        assert_eq!(
            config,
            Config {
                enabled: false,
                port: DEFAULT_PORT
            }
        );
        let config = Config::from_settings(&json!({ "agentsEnabled": true, "agentsPort": 9000 }));
        assert_eq!(
            config,
            Config {
                enabled: true,
                port: 9000
            }
        );
        for port in [json!(0), json!(70000), json!("80")] {
            let config = Config::from_settings(&json!({ "agentsPort": port }));
            assert_eq!(config.port, DEFAULT_PORT);
        }
    }

    #[test]
    fn tokens_compare_whole() {
        let token = new_token();
        assert_eq!(token.len(), 64);
        assert!(same_secret(&token, &token));
        assert!(!same_secret(&token[..63], &token));
        assert!(!same_secret(&format!("{token}0"), &token));
        assert!(!same_secret("", ""), "no token, no access");
        assert_ne!(new_token(), token);
    }
}
