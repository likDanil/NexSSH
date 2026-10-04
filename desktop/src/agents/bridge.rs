//! `NexSSH mcp` (or `--mcp`): an MCP server on standard input and output, for agents that start their
//! servers as programs (Claude Code, Claude Desktop, Codex, Cursor…). It hands every message
//! to the running NexSSH over HTTP with the token from the OS keychain, so an agent's
//! configuration names this program and holds no secret.
//!
//! Agents start their servers when they start, NexSSH or not: until a tool is called, the
//! bridge answers what needs no state itself (`initialize`, `tools/list`…). A tool call
//! starts NexSSH if it is not running (and AI agents are turned on in its settings), and
//! the bridge then opens an MCP session for the client with the `initialize` it was sent.
//! When NexSSH restarts, the session is opened again the same way.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{self, HeaderValue};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use nexssh_core::secrets::Secrets;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc};
use tokio::task::AbortHandle;

use super::http::SESSION_HEADER;
use super::{Config, TOKEN_ACCOUNT, rpc};
use crate::settings::Settings;

/// How long a NexSSH that the bridge started may take to listen.
const START_TIMEOUT: Duration = Duration::from_secs(30);
const INTERNAL_ERROR: i64 = -32603;

/// Runs until the agent closes standard input; returns the exit code.
pub fn run() -> i32 {
    crate::logger::init();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            log::error!("mcp bridge: {e}");
            return 1;
        }
    };
    runtime.block_on(serve());
    0
}

async fn serve() {
    let bridge = Arc::new(Bridge::default());
    let (out, mut replies) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(reply) = replies.recv().await {
            let mut line = reply.to_string();
            line.push('\n');
            if stdout.write_all(line.as_bytes()).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(_) => {
                let _ = out.send(rpc::error_message(
                    Value::Null,
                    rpc::PARSE_ERROR,
                    "Parse error",
                    None,
                ));
                continue;
            }
        };
        // A cancelled request: dropping its connection stops it in NexSSH too.
        if message.get("method").and_then(Value::as_str) == Some("notifications/cancelled") {
            if let Some(id) = message.get("params").and_then(|p| p.get("requestId"))
                && let Some(task) = bridge.running.lock().await.remove(&id.to_string())
            {
                task.abort();
            }
            continue;
        }
        let id = message.get("id").map(Value::to_string);
        let (bridge_, out_) = (Arc::clone(&bridge), out.clone());
        // Registered before the task can finish and take itself out.
        let mut running = bridge.running.lock().await;
        let task = tokio::spawn(async move {
            let id = message.get("id").map(Value::to_string);
            if let Some(reply) = bridge_.handle(message).await {
                let _ = out_.send(reply);
            }
            if let Some(id) = id {
                bridge_.running.lock().await.remove(&id);
            }
        });
        if let Some(id) = id {
            running.insert(id, task.abort_handle());
        }
    }
    // The agent is gone: what it waits for (an answer from the user, say) no longer matters.
    for (_, task) in bridge.running.lock().await.drain() {
        task.abort();
    }
    drop(out);
    let _ = writer.await;
}

#[derive(Default)]
struct Bridge {
    state: Mutex<State>,
    /// Requests being answered, by id (as JSON text), so they can be cancelled.
    running: Mutex<HashMap<String, AbortHandle>>,
}

#[derive(Default)]
struct State {
    /// The client's `initialize`, sent to NexSSH whenever a new MCP session is needed.
    initialize: Option<Value>,
    session: Option<String>,
    /// The protocol revision `initialize` agreed on.
    protocol: Option<String>,
    token: Option<String>,
}

/// Why a message did not reach NexSSH.
#[derive(Debug)]
enum Failure {
    /// Nothing listens: NexSSH is not running, or AI agents are turned off.
    Unreachable,
    Unauthorized,
    Other(String),
}

struct Answer {
    status: StatusCode,
    session: Option<String>,
    body: Option<Value>,
}

impl Bridge {
    async fn handle(&self, message: Value) -> Option<Value> {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let id = message.get("id").cloned();
        if method == "initialize" {
            let mut state = self.state.lock().await;
            state.initialize = Some(message.clone());
            state.session = None;
        }
        let result = match self.forward(&message, &method).await {
            Err(Failure::Unreachable) if method == "tools/call" => match self.wake().await {
                Ok(()) => self.forward(&message, &method).await,
                Err(why) => return id.map(|id| tool_error(id, &why)),
            },
            other => other,
        };
        match result {
            Ok(answer) => answer.body,
            Err(Failure::Unreachable) => {
                let id = id?;
                let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
                Some(match rpc::local(&method, &params) {
                    Some(Ok(result)) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    _ => rpc::error_message(id, INTERNAL_ERROR, &not_running(), None),
                })
            }
            Err(Failure::Unauthorized) => id.map(|id| {
                rpc::error_message(
                    id,
                    INTERNAL_ERROR,
                    "NexSSH refused its token: open NexSSH, Settings → AI agents, and turn them off and on again.",
                    None,
                )
            }),
            Err(Failure::Other(why)) => id.map(|id| rpc::error_message(id, INTERNAL_ERROR, &why, None)),
        }
    }

    /// Sends a message to NexSSH, first opening an MCP session for the client if it uses
    /// sessions and has none (NexSSH started or restarted since its `initialize`).
    async fn forward(&self, message: &Value, method: &str) -> Result<Answer, Failure> {
        if method != "initialize" {
            self.ensure_session().await?;
        }
        let answer = self.post(message).await?;
        if answer.status == StatusCode::NOT_FOUND
            && method != "initialize"
            && self.has_session().await
        {
            // NexSSH no longer knows the session: a new one, then once more.
            self.state.lock().await.session = None;
            self.ensure_session().await?;
            return self.post(message).await;
        }
        if method == "initialize" {
            let mut state = self.state.lock().await;
            state.session = answer.session.clone();
            state.protocol = answer
                .body
                .as_ref()
                .and_then(|b| b.pointer("/result/protocolVersion"))
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        Ok(answer)
    }

    async fn has_session(&self) -> bool {
        self.state.lock().await.session.is_some()
    }

    async fn ensure_session(&self) -> Result<(), Failure> {
        let initialize = {
            let state = self.state.lock().await;
            if state.session.is_some() {
                return Ok(());
            }
            match &state.initialize {
                Some(initialize) => initialize.clone(),
                // A client of the 2026 revision: no sessions.
                None => return Ok(()),
            }
        };
        let answer = self.post(&initialize).await?;
        let mut state = self.state.lock().await;
        state.session = answer.session;
        drop(state);
        let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let _ = self.post(&initialized).await;
        Ok(())
    }

    async fn post(&self, message: &Value) -> Result<Answer, Failure> {
        let config = config();
        let (session, protocol, token) = {
            let state = self.state.lock().await;
            (
                state.session.clone(),
                state.protocol.clone(),
                state.token.clone(),
            )
        };
        let token = match token {
            Some(token) => token,
            None => self.read_token().await.ok_or(Failure::Unauthorized)?,
        };
        let protocol = rpc::version_of(message).map(str::to_string).or(protocol);
        let mut answer = post(
            config.port,
            &token,
            session.as_deref(),
            protocol.as_deref(),
            message,
        )
        .await;
        if matches!(answer, Err(Failure::Unauthorized)) {
            // The token may have been renewed since it was read.
            if let Some(fresh) = self.read_token().await.filter(|t| *t != token) {
                answer = post(
                    config.port,
                    &fresh,
                    session.as_deref(),
                    protocol.as_deref(),
                    message,
                )
                .await;
            }
        }
        answer
    }

    async fn read_token(&self) -> Option<String> {
        let token = tokio::task::spawn_blocking(|| {
            Secrets::system()
                .get(TOKEN_ACCOUNT)
                .ok()
                .flatten()
                .map(|t| t.to_string())
        })
        .await
        .ok()
        .flatten()?;
        self.state.lock().await.token = Some(token.clone());
        Some(token)
    }

    /// Starts NexSSH and waits until it listens; `Err` says why that cannot be.
    async fn wake(&self) -> Result<(), String> {
        let config = config();
        if !config.enabled {
            return Err(turned_off());
        }
        if !reachable(config.port).await {
            log::info!("mcp bridge: starting NexSSH");
            start_nexssh().map_err(|e| format!("NexSSH could not be started: {e}"))?;
            let deadline = tokio::time::Instant::now() + START_TIMEOUT;
            while !reachable(config.port).await {
                if tokio::time::Instant::now() > deadline {
                    return Err(not_running());
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        Ok(())
    }
}

fn config() -> Config {
    let settings = nexssh_core::default_data_dir()
        .map(|dir| Settings::load(&dir).value().clone())
        .unwrap_or(Value::Null);
    Config::from_settings(&settings)
}

fn turned_off() -> String {
    "AI agents are turned off in NexSSH. The user can turn them on in NexSSH: Settings → AI agents."
        .into()
}

fn not_running() -> String {
    if config().enabled {
        "NexSSH is not running. The user can start it, then try again.".into()
    } else {
        turned_off()
    }
}

/// A tool's failure as a result the agent reads (rather than a protocol error).
fn tool_error(id: Value, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "resultType": "complete", "content": [{ "type": "text", "text": text }], "isError": true }
    })
}

async fn reachable(port: u16) -> bool {
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .is_ok_and(|r| r.is_ok())
}

/// Starts NexSSH on its own: not a child the agent would stop with the bridge, and without
/// the bridge's standard input and output.
fn start_nexssh() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let command = || {
        let mut command = std::process::Command::new(&exe);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    };
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::CommandExt;
        use windows::Win32::Foundation::{
            HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation,
        };
        // Windows hands a new process every inheritable handle of this one, which includes the
        // pipes the agent gave the bridge: NexSSH would hold them open after the bridge exits,
        // and the agent would never see their end.
        let handles = [
            std::io::stdin().as_raw_handle(),
            std::io::stdout().as_raw_handle(),
            std::io::stderr().as_raw_handle(),
        ];
        for handle in handles {
            // SAFETY: handles of this process, only their inheritance changes.
            let _ = unsafe {
                SetHandleInformation(HANDLE(handle), HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0))
            };
        }
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        // Outside the agent's job, so stopping the agent does not stop NexSSH; a job that
        // forbids that still lets NexSSH start inside it.
        if command()
            .creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
        command().creation_flags(flags).spawn().map(drop)
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command().process_group(0).spawn().map(drop)
    }
}

/// Aborts the connection's task when the request is dropped (cancelled), which closes the
/// connection, so NexSSH stops working on it.
struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn post(
    port: u16,
    token: &str,
    session: Option<&str>,
    protocol: Option<&str>,
    message: &Value,
) -> Result<Answer, Failure> {
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|_| Failure::Unreachable)?;
    let _ = stream.set_nodelay(true);
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| Failure::Other(e.to_string()))?;
    let _connection = AbortOnDrop(tokio::spawn(connection).abort_handle());

    let mut request = Request::post("/mcp")
        .header(header::HOST, format!("127.0.0.1:{port}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(method) = message.get("method").and_then(Value::as_str) {
        request = request.header("mcp-method", method);
    }
    if let Some(name) = message
        .pointer("/params/name")
        .and_then(Value::as_str)
        .filter(|n| n.is_ascii())
    {
        request = request.header("mcp-name", name);
    }
    if let Some(session) = session {
        request = request.header(SESSION_HEADER, session);
    }
    if let Some(protocol) = protocol {
        request = request.header("mcp-protocol-version", protocol);
    }
    let request = request
        .body(Full::new(Bytes::from(message.to_string())))
        .map_err(|e| Failure::Other(e.to_string()))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| Failure::Other(e.to_string()))?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED {
        return Err(Failure::Unauthorized);
    }
    let session = response
        .headers()
        .get(SESSION_HEADER)
        .and_then(|v: &HeaderValue| v.to_str().ok())
        .map(str::to_string);
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|e| Failure::Other(e.to_string()))?
        .to_bytes();
    let body = if body.is_empty() {
        None
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(body) => Some(body),
            Err(_) => {
                let text = String::from_utf8_lossy(&body).into_owned();
                return Err(Failure::Other(format!("NexSSH answered {status}: {text}")));
            }
        }
    };
    Ok(Answer {
        status,
        session,
        body,
    })
}
