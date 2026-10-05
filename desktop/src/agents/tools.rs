//! The tools agents call, and what they tell agents (in English: agents read it, not people).
//!
//! A tool on a server goes through the same steps: find the server among those shared with
//! agents, note it in the activity, ask the user when the server says so, find a connected
//! session of it (or have the page open a tab and wait for it to connect), then do the work
//! on that session's connection.

use std::fmt::Write as _;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nexssh_core::exec::{Captured, ExecOptions, ExecOutput, Exit};
use nexssh_core::sftp::{self, EntryKind, Sftp};
use nexssh_core::{Server, SessionId};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use super::rpc::{INVALID_PARAMS, RpcError};
use super::{ActivityStatus, Agents, Caller, Screen};
use crate::commands::AppState;

/// How long a question waits for the user.
pub(super) const ANSWER_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long a tab opened for an agent may take to connect (with the user typing a password).
const OPEN_TIMEOUT: Duration = Duration::from_secs(150);
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_TIMEOUT_SECS: u64 = 600;
/// Kept of each of a command's stdout and stderr: about 8k tokens each at most.
const MAX_OUTPUT: usize = 32 * 1024;
const READ_DEFAULT: u64 = 100_000;
const READ_MAX: u64 = 1_000_000;
const WRITE_MAX: usize = 16 * 1024 * 1024;
const LIST_MAX: usize = 1000;
/// The most lines of a terminal an agent asks for, and the most text it gets (the end).
const SCREEN_LINES_MAX: u64 = 5000;
const SCREEN_MAX: usize = 64 * 1024;

pub(super) const DECLINED: &str = "The user declined this in NexSSH. Do not try it again; ask the \
user what to do instead.";
pub(super) const NOT_ANSWERED: &str = "NexSSH stopped waiting for the user's answer (AI agents \
were turned off).";
pub(super) const NOT_ANSWERED_IN_TIME: &str = "The user did not answer in NexSSH within 5 minutes.";
pub(super) const PAGE_SILENT: &str = "NexSSH's window did not answer; try again in a moment.";
const NO_SERVERS: &str = "No servers are shared with AI agents. The user can share one in \
NexSSH: edit the server and choose what AI agents may do there.";

pub(super) fn not_shared(name: &str) -> String {
    format!(
        "{name} is not shared with AI agents. The user can allow it in NexSSH: edit the server \
         and choose what AI agents may do there."
    )
}

/// The tools, as `tools/list` describes them.
pub(super) fn list() -> Vec<Value> {
    let server = json!({
        "type": "string",
        "description": "The server's name, as list_servers gives it (may be left out when one server is shared)."
    });
    let remote_path = |what: &str| {
        json!({
            "type": "string",
            "description": format!("{what} on the server: absolute, or relative to the login folder (~ works).")
        })
    };
    vec![
        json!({
            "name": "list_servers",
            "title": "List servers",
            "description": "The servers the user shared with AI agents in NexSSH: name, address, whether NexSSH is connected to it, and whether commands need the user's approval. Call this first.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": { "title": "List servers", "readOnlyHint": true, "openWorldHint": false }
        }),
        json!({
            "name": "run_command",
            "title": "Run a command",
            "description": "Runs a shell command on a server, like `ssh server 'command'`, over the user's SSH connection in NexSSH, and returns its exit code, stdout and stderr (long output keeps its beginning and end). There is no terminal and no input other than `stdin`, so use non-interactive commands. On servers set to ask, the user approves the command in NexSSH first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "server": server,
                    "command": { "type": "string", "minLength": 1, "description": "The command line, run by the user's login shell on the server." },
                    "timeout_secs": { "type": "integer", "minimum": 1, "maximum": MAX_TIMEOUT_SECS, "default": DEFAULT_TIMEOUT_SECS, "description": "Stop the command after this many seconds." },
                    "stdin": { "type": "string", "description": "Text for the command's standard input, which then ends." }
                },
                "required": ["command"],
                "additionalProperties": false
            },
            "annotations": { "title": "Run a command", "destructiveHint": true, "openWorldHint": true }
        }),
        json!({
            "name": "read_file",
            "title": "Read a file",
            "description": "Reads a text file on a server over SFTP (a part of it with offset and length).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "server": server,
                    "path": remote_path("The file"),
                    "offset": { "type": "integer", "minimum": 0, "default": 0, "description": "Where to start, in bytes." },
                    "length": { "type": "integer", "minimum": 1, "maximum": READ_MAX, "default": READ_DEFAULT, "description": "How many bytes to read at most." }
                },
                "required": ["path"],
                "additionalProperties": false
            },
            "annotations": { "title": "Read a file", "readOnlyHint": true, "openWorldHint": false }
        }),
        json!({
            "name": "write_file",
            "title": "Write a file",
            "description": "Creates a text file on a server, or replaces its whole content (its permissions stay), over SFTP. The folder must exist. On servers set to ask, the user approves it in NexSSH first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "server": server,
                    "path": remote_path("The file"),
                    "content": { "type": "string", "description": "The file's whole new content." }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            },
            "annotations": { "title": "Write a file", "destructiveHint": true, "idempotentHint": true, "openWorldHint": false }
        }),
        json!({
            "name": "list_directory",
            "title": "List a folder",
            "description": "Lists a folder on a server: type and permissions, size in bytes, modification time (UTC) and name of each entry.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "server": server,
                    "path": remote_path("The folder (the login folder when left out)")
                },
                "additionalProperties": false
            },
            "annotations": { "title": "List a folder", "readOnlyHint": true, "openWorldHint": false }
        }),
        json!({
            "name": "terminal_read",
            "title": "Read a terminal",
            "description": "What the user's terminal tab of a server in NexSSH shows right now, as plain text the way the user sees it (a long line the terminal wrapped is one line), or its last `lines` lines with the scrollback. Use it when the user refers to something in their terminal: an error, a program's output, where a command stopped. It runs nothing and opens no tab.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "server": server,
                    "lines": { "type": "integer", "minimum": 1, "maximum": SCREEN_LINES_MAX, "description": "Instead of the screen: this many last lines, the scrollback included." },
                    "tab": { "type": "integer", "minimum": 1, "description": "Which of the server's tabs, 1 being the first in the tab bar; by default the one the user is in, else the last opened." }
                },
                "additionalProperties": false
            },
            "annotations": { "title": "Read a terminal", "readOnlyHint": true, "openWorldHint": false }
        }),
        json!({
            "name": "upload",
            "title": "Upload to a server",
            "description": "Copies a file or a whole folder from this computer to a folder on a server, at full speed. Files with the same names are replaced; folders merge with existing ones. On servers set to ask, the user approves it in NexSSH first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "server": server,
                    "local_path": { "type": "string", "description": "The absolute path of a file or folder on this computer." },
                    "remote_dir": remote_path("The folder to put it in (the login folder when left out)")
                },
                "required": ["local_path"],
                "additionalProperties": false
            },
            "annotations": { "title": "Upload to a server", "destructiveHint": true, "openWorldHint": false }
        }),
        json!({
            "name": "download",
            "title": "Download from a server",
            "description": "Copies a file or a whole folder from a server to a folder on this computer, at full speed, and returns where it went. Nothing is overwritten: a name that is taken gets a number.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "server": server,
                    "remote_path": remote_path("The file or folder"),
                    "local_dir": { "type": "string", "description": "The absolute path of a folder on this computer (the Downloads folder when left out)." }
                },
                "required": ["remote_path"],
                "additionalProperties": false
            },
            "annotations": { "title": "Download from a server", "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false }
        }),
    ]
}

/// What a tool tells the agent.
struct Outcome {
    text: String,
    error: bool,
}

impl Outcome {
    fn ok(text: impl Into<String>) -> Outcome {
        Outcome {
            text: text.into(),
            error: false,
        }
    }

    fn error(text: impl Into<String>) -> Outcome {
        Outcome {
            text: text.into(),
            error: true,
        }
    }
}

/// How work on a server ended.
struct Finished {
    /// For the agent.
    text: String,
    /// Why it failed, briefly, for the activity; `None` when it worked.
    failure: Option<String>,
    exit_code: Option<i64>,
}

impl Finished {
    fn ok(text: impl Into<String>) -> Finished {
        Finished {
            text: text.into(),
            failure: None,
            exit_code: None,
        }
    }

    fn err(message: impl Into<String>) -> Finished {
        let message = message.into();
        Finished {
            text: message.clone(),
            failure: Some(message),
            exit_code: None,
        }
    }
}

impl From<Result<String, String>> for Finished {
    fn from(result: Result<String, String>) -> Finished {
        match result {
            Ok(text) => Finished::ok(text),
            Err(message) => Finished::err(message),
        }
    }
}

pub(super) async fn call(
    app: &AppHandle,
    caller: &Caller,
    params: &Value,
) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let none = json!({});
    let args = params
        .get("arguments")
        .filter(|a| a.is_object())
        .unwrap_or(&none);
    let outcome = match name {
        "list_servers" => list_servers(app),
        "run_command" => run_command(app, caller, args).await,
        "read_file" => read_file(app, caller, args).await,
        "write_file" => write_file(app, caller, args).await,
        "list_directory" => list_directory(app, caller, args).await,
        "terminal_read" => terminal_read(app, caller, args).await,
        "upload" => upload(app, caller, args).await,
        "download" => download(app, caller, args).await,
        _ => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("Unknown tool: {name}"),
            ));
        }
    };
    Ok(json!({
        "resultType": "complete",
        "content": [{ "type": "text", "text": outcome.text }],
        "isError": outcome.error,
    }))
}

// ---- servers --------------------------------------------------------------------------

fn shared_servers(app: &AppHandle) -> Vec<Server> {
    app.state::<AppState>()
        .core
        .store
        .snapshot()
        .servers
        .into_iter()
        .filter(|s| !s.agents.is_off())
        .collect()
}

fn list_servers(app: &AppHandle) -> Outcome {
    let servers = shared_servers(app);
    if servers.is_empty() {
        return Outcome::ok(NO_SERVERS);
    }
    let sessions = &app.state::<AppState>().core.sessions;
    let mut text = String::from("Servers shared with AI agents in NexSSH:\n");
    for s in &servers {
        let same_name = servers.iter().filter(|o| o.name == s.name).count() > 1;
        let id = if same_name {
            format!(" [id {}]", s.id)
        } else {
            String::new()
        };
        let group = if s.group.is_empty() {
            String::new()
        } else {
            format!(", group {}", s.group)
        };
        let state = if sessions.connected(&s.id).is_some() {
            "connected"
        } else {
            "not connected (NexSSH connects on first use)"
        };
        let access = match s.agents {
            nexssh_core::AgentAccess::Allow => "full access",
            _ => "commands and changes need the user's approval",
        };
        let _ = writeln!(
            text,
            "- {}{id}: {}{group}; {state}; {access}",
            s.name,
            s.destination()
        );
    }
    Outcome::ok(text.trim_end())
}

/// The shared server the agent names: by id, name, alias or host.
fn resolve(app: &AppHandle, args: &Value) -> Result<Server, String> {
    let wanted = text_arg(args, "server").unwrap_or_default();
    let shared = shared_servers(app);
    if wanted.is_empty() {
        return match shared.as_slice() {
            [only] => Ok(only.clone()),
            [] => Err(NO_SERVERS.to_string()),
            _ => Err(format!(
                "Say which server: {}.",
                shared
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        };
    }
    if let Some(found) = find(&shared, wanted) {
        return Ok(found.clone());
    }
    let all = app.state::<AppState>().core.store.snapshot().servers;
    match find(&all, wanted) {
        Some(other) => Err(not_shared(&other.name)),
        None if shared.is_empty() => Err(NO_SERVERS.to_string()),
        None => Err(format!(
            "There is no shared server named {wanted}. Shared servers: {}.",
            shared
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn find<'a>(servers: &'a [Server], wanted: &str) -> Option<&'a Server> {
    servers
        .iter()
        .find(|s| s.id == wanted)
        .or_else(|| servers.iter().find(|s| s.name.eq_ignore_ascii_case(wanted)))
        .or_else(|| servers.iter().find(|s| s.alias.as_deref() == Some(wanted)))
        .or_else(|| {
            servers
                .iter()
                .find(|s| s.destination().eq_ignore_ascii_case(wanted))
        })
        .or_else(|| {
            let mut by_host = servers
                .iter()
                .filter(|s| s.host.eq_ignore_ascii_case(wanted));
            let first = by_host.next();
            if by_host.next().is_none() {
                first
            } else {
                None
            }
        })
}

fn text_arg<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str).map(str::trim)
}

fn number_arg(args: &Value, name: &str) -> Option<u64> {
    let value = args.get(name)?;
    value
        .as_u64()
        .or_else(|| value.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

// ---- the steps every tool on a server takes -------------------------------------------

/// An entry in the activity that ends as failed unless it is finished: the agent may stop
/// waiting (its request is dropped) at any step.
struct Noted<'a> {
    agents: &'a Agents,
    id: u64,
    finished: bool,
}

impl Noted<'_> {
    fn end(&mut self, status: ActivityStatus, error: Option<String>, exit_code: Option<i64>) {
        self.finished = true;
        self.agents.update(self.id, |a| {
            a.status = status;
            a.error = error.map(|e| clip(&e, 300));
            a.exit_code = exit_code;
        });
    }
}

impl Drop for Noted<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.agents.update(self.id, |a| {
                a.status = ActivityStatus::Failed;
                a.error = Some("the agent stopped waiting".into());
            });
        }
    }
}

/// Runs `work` on a connected session of the server the agent names. `ask`: the user must
/// allow it first (on servers set to ask), with this preview.
async fn on_server<W, F>(
    app: &AppHandle,
    caller: &Caller,
    args: &Value,
    tool: &str,
    detail: &str,
    ask: Option<Option<String>>,
    work: W,
) -> Outcome
where
    W: FnOnce(SessionId) -> F,
    F: Future<Output = Finished>,
{
    let server = match resolve(app, args) {
        Ok(server) => server,
        Err(message) => return Outcome::error(message),
    };
    let agents = app.state::<Agents>();
    let mut noted = Noted {
        agents: &agents,
        id: agents.begin(caller, &server, tool, detail),
        finished: false,
    };
    if let Some(preview) = ask {
        // The activity's status says it; what the agent is told is meant for the agent.
        if let Err(why) = agents.ask(caller, &server, tool, detail, preview).await {
            noted.end(ActivityStatus::Denied, None, None);
            return Outcome::error(why);
        }
        // The user may have changed the server while the question waited.
        let still = app.state::<AppState>().core.store.get(&server.id);
        if still.is_none_or(|s| s.agents.is_off()) {
            noted.end(ActivityStatus::Denied, None, None);
            return Outcome::error(not_shared(&server.name));
        }
    }
    let session = match session_for(app, caller, &server).await {
        Ok(session) => session,
        Err(message) => {
            noted.end(ActivityStatus::Failed, Some(message.clone()), None);
            return Outcome::error(message);
        }
    };
    agents.update(noted.id, |a| {
        a.status = ActivityStatus::Running;
        a.session_id = Some(session);
    });
    let finished = work(session).await;
    match finished.failure {
        None => {
            noted.end(ActivityStatus::Done, None, finished.exit_code);
            Outcome::ok(finished.text)
        }
        Some(why) => {
            noted.end(ActivityStatus::Failed, Some(why), finished.exit_code);
            Outcome::error(finished.text)
        }
    }
}

/// A connected session of the server: an existing one, or one in a tab the page opens.
async fn session_for(
    app: &AppHandle,
    caller: &Caller,
    server: &Server,
) -> Result<SessionId, String> {
    let sessions = &app.state::<AppState>().core.sessions;
    if let Some(id) = sessions.connected(&server.id) {
        return Ok(id);
    }
    let agents = app.state::<Agents>();
    let mut changes = sessions.live_changes();
    // The page's list shows the tab as awaited until this returns.
    let (_open, mut failed) = agents.open_tab(caller, &server.id);
    let deadline = tokio::time::Instant::now() + OPEN_TIMEOUT;
    loop {
        if let Some(id) = sessions.connected(&server.id) {
            break Ok(id);
        }
        tokio::select! {
            changed = changes.changed() => if changed.is_err() {
                break Err("NexSSH is closing.".to_string());
            },
            reason = &mut failed => break Err(match reason {
                Ok(reason) => format!("NexSSH could not connect to {}: {reason}", server.name),
                Err(_) => NOT_ANSWERED.to_string(),
            }),
            () = tokio::time::sleep_until(deadline) => break Err(format!(
                "{} did not connect within {} s. The user may need to sign in to it in NexSSH.",
                server.name,
                OPEN_TIMEOUT.as_secs()
            )),
        }
    }
}

async fn sftp_of(app: &AppHandle, session: SessionId) -> Result<Arc<Sftp>, String> {
    app.state::<AppState>()
        .core
        .sessions
        .sftp(session)
        .await
        .map_err(|e| e.to_string())
}

/// A path as the agent gives it, made absolute: `~` and relative paths start at the login
/// folder.
async fn remote_path(sftp: &Sftp, path: &str) -> Result<String, String> {
    let path = path.trim();
    let home = || async { sftp.home().await.map_err(|e| e.to_string()) };
    if path.is_empty() || path == "~" || path == "." {
        return home().await;
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return Ok(sftp::join(&home().await?, rest));
    }
    if !path.starts_with('/') {
        return Ok(sftp::join(&home().await?, path));
    }
    Ok(path.to_string())
}

// ---- tools ----------------------------------------------------------------------------

async fn run_command(app: &AppHandle, caller: &Caller, args: &Value) -> Outcome {
    let Some(command) = text_arg(args, "command").filter(|c| !c.is_empty()) else {
        return Outcome::error("command is required.");
    };
    let timeout = number_arg(args, "timeout_secs")
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, MAX_TIMEOUT_SECS);
    let stdin = args
        .get("stdin")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let preview = (!stdin.is_empty()).then(|| clip_lines(&stdin, 12, 1200));
    on_server(
        app,
        caller,
        args,
        "run_command",
        command,
        Some(preview),
        |session| async move {
            let options = ExecOptions {
                timeout: Duration::from_secs(timeout),
                max_output: MAX_OUTPUT,
                stdin: stdin.into_bytes(),
            };
            let started = Instant::now();
            let result = app
                .state::<AppState>()
                .core
                .sessions
                .exec(session, command, options)
                .await;
            match result {
                Ok(output) => command_result(&output, started.elapsed(), timeout),
                Err(e) => Finished::err(e.to_string()),
            }
        },
    )
    .await
}

fn command_result(output: &ExecOutput, took: Duration, timeout: u64) -> Finished {
    let mut text = match (&output.exit, output.timed_out) {
        (_, true) => format!(
            "The command did not finish within {timeout} s and was stopped (raise timeout_secs if \
             it needs longer)."
        ),
        (Some(Exit::Code(code)), _) => format!("Exit code {code} ({:.1} s)", took.as_secs_f64()),
        (Some(Exit::Signal(signal)), _) => {
            format!("Killed by signal {signal} ({:.1} s)", took.as_secs_f64())
        }
        (None, _) => format!(
            "The command ended without an exit status ({:.1} s)",
            took.as_secs_f64()
        ),
    };
    if output.stdout.total() == 0 && output.stderr.total() == 0 {
        text.push_str(if output.timed_out {
            " No output."
        } else {
            ", no output."
        });
    }
    if output.stdout.total() > 0 {
        let _ = write!(text, "\n\nstdout:\n{}", stream(&output.stdout));
    }
    if output.stderr.total() > 0 {
        let _ = write!(text, "\n\nstderr:\n{}", stream(&output.stderr));
    }
    let exit_code = match output.exit {
        Some(Exit::Code(code)) => Some(i64::from(code)),
        _ => None,
    };
    Finished {
        text,
        failure: output
            .timed_out
            .then(|| format!("stopped after {timeout} s")),
        exit_code,
    }
}

/// An output stream as text: all of it, or its beginning and end.
fn stream(captured: &Captured) -> String {
    let (head, tail) = captured.parts();
    let mut text = String::from_utf8_lossy(head).into_owned();
    if captured.omitted() > 0 {
        let _ = write!(text, "\n[… {} left out …]\n", size(captured.omitted()));
    }
    text.push_str(&String::from_utf8_lossy(&tail));
    text
}

async fn read_file(app: &AppHandle, caller: &Caller, args: &Value) -> Outcome {
    let Some(path) = text_arg(args, "path").filter(|p| !p.is_empty()) else {
        return Outcome::error("path is required.");
    };
    let offset = number_arg(args, "offset").unwrap_or(0);
    let length = number_arg(args, "length")
        .unwrap_or(READ_DEFAULT)
        .clamp(1, READ_MAX);
    on_server(
        app,
        caller,
        args,
        "read_file",
        path,
        None,
        |session| async move {
            let read = async {
                let sftp = sftp_of(app, session).await?;
                let path = remote_path(&sftp, path).await?;
                let (data, size) = sftp
                    .read_part(&path, offset, length)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>((path, data, size))
            };
            match read.await {
                Ok((path, data, size)) => file_text(&path, &data, size, offset, length).into(),
                Err(e) => Finished::err(e),
            }
        },
    )
    .await
}

fn file_text(
    path: &str,
    data: &[u8],
    size: Option<u64>,
    offset: u64,
    length: u64,
) -> Result<String, String> {
    if data.contains(&0) {
        return Err(format!(
            "{path} looks like a binary file; download copies it to this computer."
        ));
    }
    // Files such as those in /proc say they are empty, whatever they hold.
    let size = size.filter(|s| *s > 0 || data.is_empty());
    let end = offset + data.len() as u64;
    let more = match size {
        Some(size) => end < size,
        None => data.len() as u64 == length,
    };
    let header = if offset == 0 && !more {
        format!("{path} ({} bytes):", data.len())
    } else {
        let of = size.map(|s| format!(" of {s}")).unwrap_or_default();
        let next = if more {
            format!("; more from offset={end}")
        } else {
            String::new()
        };
        format!("{path}, bytes {offset}–{end}{of}{next}:")
    };
    Ok(format!("{header}\n{}", String::from_utf8_lossy(data)))
}

async fn write_file(app: &AppHandle, caller: &Caller, args: &Value) -> Outcome {
    let Some(path) = text_arg(args, "path").filter(|p| !p.is_empty()) else {
        return Outcome::error("path is required.");
    };
    let Some(content) = args.get("content").and_then(Value::as_str) else {
        return Outcome::error("content is required.");
    };
    if content.len() > WRITE_MAX {
        return Outcome::error(format!(
            "content is too large ({}); upload copies large files.",
            size(content.len() as u64)
        ));
    }
    let preview = Some(clip_lines(content, 14, 1400));
    on_server(
        app,
        caller,
        args,
        "write_file",
        path,
        Some(preview),
        |session| async move {
            let write = async {
                let sftp = sftp_of(app, session).await?;
                let path = remote_path(&sftp, path).await?;
                sftp.write_whole(&path, content.as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(format!("Wrote {} bytes to {path}.", content.len()))
            };
            write.await.into()
        },
    )
    .await
}

async fn list_directory(app: &AppHandle, caller: &Caller, args: &Value) -> Outcome {
    let path = text_arg(args, "path").unwrap_or_default();
    let shown = if path.is_empty() { "~" } else { path };
    on_server(
        app,
        caller,
        args,
        "list_directory",
        shown,
        None,
        |session| async move {
            let list = async {
                let sftp = sftp_of(app, session).await?;
                let path = remote_path(&sftp, path).await?;
                let entries = sftp.list(&path).await.map_err(|e| e.to_string())?;
                Ok::<_, String>((path, entries))
            };
            match list.await {
                Ok((path, entries)) => Finished::ok(listing(&path, &entries)),
                Err(e) => Finished::err(e),
            }
        },
    )
    .await
}

fn listing(path: &str, entries: &[sftp::Entry]) -> String {
    let mut text = match entries.len() {
        0 => return format!("{path} is empty."),
        1 => format!("{path} (1 entry):"),
        n => format!("{path} ({n} entries):"),
    };
    for entry in entries.iter().take(LIST_MAX) {
        let kind = match entry.kind {
            EntryKind::Dir => 'd',
            EntryKind::Link => 'l',
            EntryKind::File => '-',
            EntryKind::Other => '?',
        };
        let permissions = entry.permissions.as_deref().unwrap_or("?????????");
        let permissions = permissions
            .get(permissions.len().saturating_sub(9)..)
            .unwrap_or(permissions);
        let modified = entry.modified.map(utc).unwrap_or_else(|| "-".repeat(16));
        let suffix = match entry.kind {
            EntryKind::Dir => "/",
            EntryKind::Link if entry.link_to_dir => "@/",
            EntryKind::Link => "@",
            _ => "",
        };
        let _ = write!(
            text,
            "\n{kind}{permissions} {:>12} {modified} {}{suffix}",
            entry.size, entry.name
        );
    }
    if entries.len() > LIST_MAX {
        let _ = write!(text, "\n… and {} more", entries.len() - LIST_MAX);
    }
    text
}

async fn terminal_read(app: &AppHandle, caller: &Caller, args: &Value) -> Outcome {
    let server = match resolve(app, args) {
        Ok(server) => server,
        Err(message) => return Outcome::error(message),
    };
    let lines = number_arg(args, "lines").map(|n| n.clamp(1, SCREEN_LINES_MAX));
    let tab = number_arg(args, "tab").filter(|n| *n >= 1);
    let agents = app.state::<Agents>();
    let mut noted = Noted {
        agents: &agents,
        id: agents.begin(caller, &server, "terminal_read", ""),
        finished: false,
    };
    agents.update(noted.id, |a| a.status = ActivityStatus::Running);
    match agents.read_screen(&server.id, tab, lines).await {
        Ok(screen) => {
            noted.end(ActivityStatus::Done, None, None);
            Outcome::ok(screen_text(&screen, lines.is_some()))
        }
        Err(reason) => {
            let message = screen_error(&server.name, &reason, tab);
            noted.end(ActivityStatus::Failed, Some(message.clone()), None);
            Outcome::error(message)
        }
    }
}

fn screen_error(name: &str, reason: &str, tab: Option<u64>) -> String {
    match reason {
        "noTab" => format!(
            "No tab of {name} is open in NexSSH, so there is no terminal to read (run_command \
             opens one)."
        ),
        "noSuchTab" => format!(
            "{name} has no tab number {}; leave tab out for the one the user is in.",
            tab.unwrap_or(0)
        ),
        "notReady" => "The tab's terminal is still opening; try again in a moment.".to_string(),
        other => other.to_string(),
    }
}

fn screen_text(s: &Screen, lines: bool) -> String {
    let mut text = format!(
        "{}, tab {} of {} ({}), {}×{}; the cursor is at row {}, column {}.",
        s.title, s.tab, s.tabs, s.status, s.cols, s.rows, s.cursor_row, s.cursor_col
    );
    if s.alternate {
        text.push_str(" A full-screen program has the screen (no scrollback while it runs).");
    }
    let body = keep_end(&s.text, SCREEN_MAX);
    let _ = if lines {
        write!(text, "\nLines {}–{} of {}:", s.from, s.total, s.total)
    } else if s.above > 0 {
        write!(
            text,
            "\nThe screen ({} lines above it in the scrollback; ask for lines to see them):",
            s.above
        )
    } else {
        write!(text, "\nThe screen:")
    };
    if body.is_empty() {
        text.push_str(" empty");
    } else {
        let _ = write!(text, "\n{body}");
    }
    text
}

/// The end of `text`, at most `max` bytes from the start of a line.
fn keep_end(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let rest = &text[start..];
    let rest = rest.split_once('\n').map_or(rest, |(_, after)| after);
    format!("[earlier lines left out]\n{rest}")
}

/// Stops a transfer running in its own task when the agent stops waiting for it, so the
/// transfer cleans up after itself (a dropped transfer would not).
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

async fn upload(app: &AppHandle, caller: &Caller, args: &Value) -> Outcome {
    let Some(local) = text_arg(args, "local_path").filter(|p| !p.is_empty()) else {
        return Outcome::error("local_path is required.");
    };
    let local = PathBuf::from(local);
    if !local.is_absolute() {
        return Outcome::error("local_path must be an absolute path on this computer.");
    }
    if let Err(e) = std::fs::metadata(&local) {
        return Outcome::error(format!("{}: {e}", local.display()));
    }
    let dir = text_arg(args, "remote_dir").unwrap_or_default().to_string();
    let shown_dir = if dir.is_empty() { "~" } else { dir.as_str() };
    let detail = format!("{} → {shown_dir}", local.display());
    on_server(
        app,
        caller,
        args,
        "upload",
        &detail,
        Some(None),
        |session| async move {
            let sftp = match sftp_of(app, session).await {
                Ok(sftp) => sftp,
                Err(e) => return Finished::err(e),
            };
            let dir = match remote_path(&sftp, &dir).await {
                Ok(dir) => dir,
                Err(e) => return Finished::err(e),
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let _stop = CancelOnDrop(Arc::clone(&cancel));
            let started = Instant::now();
            let task = tauri::async_runtime::spawn(async move {
                let mut total = 0;
                let result = sftp
                    .upload_path(&local, &dir, &mut |p| total = p.total, &cancel)
                    .await;
                (result, total)
            });
            match task.await {
                Ok((Ok(remote), total)) => Finished::ok(format!(
                    "Uploaded to {remote} ({}, {}).",
                    size(total),
                    rate(total, started.elapsed())
                )),
                Ok((Err(e), _)) => Finished::err(e.to_string()),
                Err(e) => Finished::err(e.to_string()),
            }
        },
    )
    .await
}

async fn download(app: &AppHandle, caller: &Caller, args: &Value) -> Outcome {
    let Some(remote) = text_arg(args, "remote_path").filter(|p| !p.is_empty()) else {
        return Outcome::error("remote_path is required.");
    };
    let dir = match text_arg(args, "local_dir").filter(|d| !d.is_empty()) {
        Some(dir) if !PathBuf::from(dir).is_absolute() => {
            return Outcome::error("local_dir must be an absolute path on this computer.");
        }
        Some(dir) => PathBuf::from(dir),
        None => sftp::download_dir(),
    };
    on_server(
        app,
        caller,
        args,
        "download",
        remote,
        None,
        |session| async move {
            let sftp = match sftp_of(app, session).await {
                Ok(sftp) => sftp,
                Err(e) => return Finished::err(e),
            };
            let remote = match remote_path(&sftp, remote).await {
                Ok(remote) => remote,
                Err(e) => return Finished::err(e),
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let _stop = CancelOnDrop(Arc::clone(&cancel));
            let started = Instant::now();
            let task = tauri::async_runtime::spawn(async move {
                let mut total = 0;
                let result = sftp
                    .download(&remote, &dir, &mut |p| total = p.total, &cancel)
                    .await;
                (result, total)
            });
            match task.await {
                Ok((Ok(local), total)) => Finished::ok(format!(
                    "Downloaded to {} ({}, {}).",
                    local.display(),
                    size(total),
                    rate(total, started.elapsed())
                )),
                Ok((Err(e), _)) => Finished::err(e.to_string()),
                Err(e) => Finished::err(e.to_string()),
            }
        },
    )
    .await
}

// ---- formatting -----------------------------------------------------------------------

/// At most `max` characters, with an ellipsis when cut.
pub(super) fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

/// The first `max_lines` lines, at most `max_chars` characters.
fn clip_lines(text: &str, max_lines: usize, max_chars: usize) -> String {
    let mut shown: String = text.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    let cut = text.lines().count() > max_lines;
    shown = clip(&shown, max_chars);
    if cut && !shown.ends_with('…') {
        shown.push_str("\n…");
    }
    shown
}

fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// How long a transfer took, and its speed when it took long enough to tell.
fn rate(bytes: u64, took: Duration) -> String {
    let secs = took.as_secs_f64();
    if secs < 0.5 {
        return format!("{secs:.1} s");
    }
    format!("{secs:.1} s, {}/s", size((bytes as f64 / secs) as u64))
}

/// `2026-10-04 12:30` (UTC) from Unix time.
fn utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_are_described_completely() {
        let tools = list();
        assert_eq!(tools.len(), 8);
        for tool in &tools {
            let name = tool["name"].as_str().unwrap();
            assert!(tool["description"].as_str().unwrap().len() > 40, "{name}");
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            assert!(tool["annotations"].is_object(), "{name}");
            if let Some(required) = tool["inputSchema"]["required"].as_array() {
                for field in required {
                    let field = field.as_str().unwrap();
                    assert!(
                        tool["inputSchema"]["properties"][field].is_object(),
                        "{name}.{field}"
                    );
                }
            }
        }
    }

    #[test]
    fn finds_servers_by_what_agents_call_them() {
        let server = |id: &str, name: &str, host: &str| Server {
            id: id.into(),
            name: name.into(),
            host: host.into(),
            user: "deploy".into(),
            ..Server::default()
        };
        let servers = vec![
            server("a1", "web-01", "10.0.0.1"),
            server("b2", "db", "10.0.0.2"),
            server("c3", "db-replica", "10.0.0.2"),
        ];
        assert_eq!(find(&servers, "b2").unwrap().id, "b2");
        assert_eq!(find(&servers, "WEB-01").unwrap().id, "a1");
        assert_eq!(find(&servers, "10.0.0.1").unwrap().id, "a1");
        assert_eq!(find(&servers, "deploy@10.0.0.1").unwrap().id, "a1");
        assert!(
            find(&servers, "10.0.0.2").is_none(),
            "two servers have that host"
        );
        assert!(find(&servers, "web").is_none());
    }

    #[test]
    fn describes_files() {
        let text = file_text("/etc/hosts", b"127.0.0.1 localhost\n", Some(20), 0, 100).unwrap();
        assert_eq!(text, "/etc/hosts (20 bytes):\n127.0.0.1 localhost\n");
        let text = file_text("/log", b"abcd", Some(10), 0, 4).unwrap();
        assert!(
            text.starts_with("/log, bytes 0–4 of 10; more from offset=4:\n"),
            "{text}"
        );
        let text = file_text("/log", b"ef", Some(10), 8, 4).unwrap();
        assert!(text.starts_with("/log, bytes 8–10 of 10:\n"), "{text}");
        // /proc files say they are empty.
        let text = file_text("/proc/x", b"abcd", Some(0), 0, 4).unwrap();
        assert!(
            text.starts_with("/proc/x, bytes 0–4; more from offset=4:"),
            "{text}"
        );
        let text = file_text("/proc/x", b"ab", Some(0), 0, 4).unwrap();
        assert!(text.starts_with("/proc/x (2 bytes):"), "{text}");
        assert!(file_text("/bin/ls", b"\x7fELF\0\0", Some(6), 0, 10).is_err());
    }

    #[test]
    fn lists_folders() {
        let entry = |name: &str, kind, size| sftp::Entry {
            name: name.into(),
            kind,
            link_to_dir: false,
            size,
            modified: Some(1_790_000_000),
            permissions: Some("drwxr-xr-x".into()),
            mode: Some(0o755),
        };
        let text = listing(
            "/srv",
            &[
                entry("app", EntryKind::Dir, 4096),
                entry("run.sh", EntryKind::File, 120),
            ],
        );
        assert_eq!(
            text,
            "/srv (2 entries):\n\
             drwxr-xr-x         4096 2026-09-21 14:13 app/\n\
             -rwxr-xr-x          120 2026-09-21 14:13 run.sh"
        );
        assert_eq!(listing("/empty", &[]), "/empty is empty.");
    }

    #[test]
    fn describes_screens() {
        let screen = Screen {
            title: "web-01".into(),
            status: "connected".into(),
            tab: 1,
            tabs: 2,
            cols: 120,
            rows: 30,
            cursor_row: 3,
            cursor_col: 15,
            alternate: false,
            above: 200,
            from: 201,
            total: 230,
            text: "$ make\nerror: no rule".into(),
        };
        let text = screen_text(&screen, false);
        assert!(text.starts_with(
            "web-01, tab 1 of 2 (connected), 120×30; the cursor is at row 3, column 15."
        ));
        assert!(text.contains("200 lines above it"), "{text}");
        assert!(text.ends_with("\n$ make\nerror: no rule"), "{text}");
        let text = screen_text(&screen, true);
        assert!(text.contains("Lines 201–230 of 230:"), "{text}");

        let long: String = (0..10_000).map(|i| format!("line {i}\n")).collect();
        let kept = keep_end(&long, 1000);
        assert!(
            kept.starts_with("[earlier lines left out]\nline "),
            "{kept}"
        );
        assert!(kept.ends_with("line 9999\n"));
        assert!(kept.len() < 1100);
        assert_eq!(keep_end("short", 1000), "short");
        assert!(screen_error("db", "noTab", None).contains("No tab of db"));
    }

    #[test]
    fn formats() {
        assert_eq!(utc(0), "1970-01-01 00:00");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00");
        assert_eq!(size(999), "999 bytes");
        assert_eq!(size(1_500), "1.5 KB");
        assert_eq!(size(2_500_000), "2.5 MB");
        assert_eq!(clip("hello", 3), "hel…");
        assert_eq!(clip("привет", 10), "привет");
        assert_eq!(clip_lines("a\nb\nc", 2, 100), "a\nb\n…");
    }
}
