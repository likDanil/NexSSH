//! MCP's JSON-RPC messages: who is calling, which protocol revision, and the answers.
//!
//! Two generations of clients are served side by side:
//!
//! * revisions 2024-11-05 to 2025-11-25 start with `initialize`, which gets an MCP session
//!   (the `Mcp-Session-Id` header) that later requests carry. A session NexSSH does not know
//!   (it restarted, the token changed) gets 404, after which clients start a new one.
//! * revision 2026-07-28 has no handshake and no sessions: every request carries its protocol
//!   version and the client's name in `_meta`, and `server/discover` describes the server.
//!
//! What needs no state (`initialize`, `server/discover`, `ping`, `tools/list`) is answered by
//! [`local`], which the stdio bridge uses too while NexSSH is not running.

use hyper::StatusCode;
use serde_json::{Map, Value, json};
use tauri::{AppHandle, Manager};

use super::http::Meta;
use super::{Agents, Caller, tools};

pub(super) const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
pub(super) const INVALID_PARAMS: i64 = -32602;
const SESSION_NOT_FOUND: i64 = -32001;
const HEADER_MISMATCH: i64 = -32020;
const UNSUPPORTED_VERSION: i64 = -32022;

/// Newest first.
pub(super) const SUPPORTED: [&str; 5] = [
    "2026-07-28",
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
];
/// The first revision without `initialize` and sessions.
const STATELESS: &str = "2026-07-28";
/// What `initialize` answers when a client asks for a revision NexSSH does not know.
const LATEST_WITH_SESSIONS: &str = "2025-11-25";

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT: &str = "io.modelcontextprotocol/clientInfo";

/// How agents should use NexSSH, sent with `initialize` and `server/discover`.
const INSTRUCTIONS: &str = "NexSSH is the user's SSH client; these tools work on the servers the \
user shared with AI agents in NexSSH. Call list_servers first and name servers by their names. \
Everything runs over the user's own SSH connections: a server that is not connected gets a tab \
in NexSSH that signs in, where the user may have to type a password or a code, so a first call \
can take a while. On servers set to ask, commands and changes wait until the user allows them \
in NexSSH; if the user declines, do not try the same again, ask the user instead. run_command \
has no terminal and no input: use non-interactive forms (sudo -n, apt-get -y, \
systemctl --no-pager, git --no-pager, journalctl --no-pager -n 200) and avoid programs that \
wait for keys or never end (top, less, vim, tail -f, watch). Use read_file, write_file and \
list_directory for files on a server, and upload and download to move files and folders \
between this computer and a server. When the user talks about something in their terminal (an \
error, a program's output), terminal_read shows what their tab of the server shows.";

/// A JSON-RPC error.
#[derive(Debug)]
pub(super) struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }
}

pub(super) fn error_message(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

/// The HTTP answer to a POST.
#[derive(Debug)]
pub(super) struct Reply {
    pub status: StatusCode,
    pub body: Option<Value>,
    /// A new MCP session, for the `Mcp-Session-Id` header.
    pub session: Option<String>,
}

impl Reply {
    fn accepted() -> Reply {
        Reply {
            status: StatusCode::ACCEPTED,
            body: None,
            session: None,
        }
    }

    fn error(
        status: StatusCode,
        id: Value,
        code: i64,
        message: &str,
        data: Option<Value>,
    ) -> Reply {
        Reply {
            status,
            body: Some(error_message(id, code, message, data)),
            session: None,
        }
    }
}

/// One message, read but not answered yet.
enum Step {
    Done(Reply),
    /// A tool runs; what is needed to answer afterwards.
    Call {
        id: Value,
        caller: Caller,
        params: Value,
    },
}

/// Answers a POSTed message, or a batch of them (revision 2025-03-26 allowed batches).
pub(super) async fn handle(app: &AppHandle, meta: &Meta, message: Value) -> Reply {
    let agents = app.state::<Agents>();
    let items = match message {
        Value::Array(items) if !items.is_empty() => items,
        message => return run(app, prepare(&agents, meta, message)).await,
    };
    let mut answers = Vec::new();
    let mut session = None;
    for item in items {
        let reply = run(app, prepare(&agents, meta, item)).await;
        session = session.or(reply.session);
        answers.extend(reply.body);
    }
    if answers.is_empty() {
        return Reply::accepted();
    }
    Reply {
        status: StatusCode::OK,
        body: Some(Value::Array(answers)),
        session,
    }
}

async fn run(app: &AppHandle, step: Step) -> Reply {
    match step {
        Step::Done(reply) => reply,
        Step::Call { id, caller, params } => {
            let answer = match tools::call(app, &caller, &params).await {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err(e) => error_message(id, e.code, &e.message, e.data),
            };
            Reply {
                status: StatusCode::OK,
                body: Some(answer),
                session: None,
            }
        }
    }
}

fn prepare(agents: &Agents, meta: &Meta, message: Value) -> Step {
    let Value::Object(object) = message else {
        return Step::Done(Reply::error(
            StatusCode::BAD_REQUEST,
            Value::Null,
            INVALID_REQUEST,
            "Invalid Request",
            None,
        ));
    };
    let id = object.get("id").cloned();
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        // A response: NexSSH sends no requests, so there is nothing to match it with.
        if object.contains_key("result") || object.contains_key("error") {
            return Step::Done(Reply::accepted());
        }
        return Step::Done(Reply::error(
            StatusCode::BAD_REQUEST,
            id.unwrap_or(Value::Null),
            INVALID_REQUEST,
            "Invalid Request",
            None,
        ));
    };
    let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
    let meta_object = params.get("_meta").and_then(Value::as_object);
    let body_version = meta_object
        .and_then(|m| m.get(META_VERSION))
        .and_then(Value::as_str);
    if let (Some(header), Some(body)) = (meta.protocol.as_deref(), body_version)
        && header != body
    {
        return Step::Done(Reply::error(
            StatusCode::BAD_REQUEST,
            id.unwrap_or(Value::Null),
            HEADER_MISMATCH,
            "Header mismatch: MCP-Protocol-Version differs from the request's _meta",
            None,
        ));
    }
    let version = body_version.or(meta.protocol.as_deref());
    if method != "initialize"
        && let Some(version) = version
        && !SUPPORTED.contains(&version)
    {
        return Step::Done(Reply::error(
            StatusCode::BAD_REQUEST,
            id.unwrap_or(Value::Null),
            UNSUPPORTED_VERSION,
            "Unsupported protocol version",
            Some(json!({ "supported": SUPPORTED, "requested": version })),
        ));
    }
    let stateless = version.is_some_and(|v| v >= STATELESS);

    // A notification (`notifications/initialized`, `notifications/cancelled`…): nothing to
    // answer.
    let Some(id) = id else {
        return Step::Done(Reply::accepted());
    };

    let mut session = None;
    let caller = if method == "initialize" {
        let name = client_name(params.get("clientInfo"));
        let id = agents.add_client(&name);
        session = Some(id.clone());
        Caller { key: id, name }
    } else if let (Some(known), false) = (meta.session.as_deref(), stateless) {
        match agents.client(known) {
            Some(caller) => caller,
            None => {
                return Step::Done(Reply::error(
                    StatusCode::NOT_FOUND,
                    id,
                    SESSION_NOT_FOUND,
                    "Session not found: start a new one",
                    None,
                ));
            }
        }
    } else {
        agents.stateless(&client_name(meta_object.and_then(|m| m.get(META_CLIENT))))
    };

    let result = match local(method, &params) {
        Some(result) => result,
        None if method == "tools/call" => return Step::Call { id, caller, params },
        None => Err(RpcError::new(
            METHOD_NOT_FOUND,
            format!("Method not found: {method}"),
        )),
    };
    let (status, body) = match result {
        Ok(result) => (
            StatusCode::OK,
            json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        ),
        Err(e) => {
            // The 2026 transport reports unknown methods with 404 too.
            let status = if e.code == METHOD_NOT_FOUND && stateless {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::OK
            };
            (status, error_message(id, e.code, &e.message, e.data))
        }
    };
    Step::Done(Reply {
        status,
        body: Some(body),
        session,
    })
}

/// Methods that need neither NexSSH's state nor the caller.
pub(super) fn local(method: &str, params: &Value) -> Option<Result<Value, RpcError>> {
    let result = match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = asked
                .filter(|v| SUPPORTED.contains(v) && *v < STATELESS)
                .unwrap_or(LATEST_WITH_SESSIONS);
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": server_info(),
                "instructions": INSTRUCTIONS,
            })
        }
        "server/discover" => json!({
            "resultType": "complete",
            "supportedVersions": SUPPORTED,
            "capabilities": { "tools": { "listChanged": false } },
            "_meta": { "io.modelcontextprotocol/serverInfo": server_info() },
            "instructions": INSTRUCTIONS,
            "ttlMs": 3_600_000,
            "cacheScope": "public",
        }),
        "ping" => json!({ "resultType": "complete" }),
        "tools/list" => json!({
            "resultType": "complete",
            "tools": tools::list(),
            "ttlMs": 3_600_000,
            "cacheScope": "public",
        }),
        _ => return None,
    };
    Some(Ok(result))
}

fn server_info() -> Value {
    json!({ "name": "nexssh", "title": "NexSSH", "version": env!("CARGO_PKG_VERSION") })
}

/// The name to show for a client: its title, or a known client's usual name, or its name.
fn client_name(info: Option<&Value>) -> String {
    let field = |key: &str| {
        info.and_then(|i| i.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let name = match (field("title"), field("name")) {
        (Some(title), _) => title.to_string(),
        (None, Some(name)) => known_name(name).unwrap_or(name).to_string(),
        (None, None) => "AI agent".to_string(),
    };
    let clean: String = name.chars().filter(|c| !c.is_control()).take(48).collect();
    if clean.trim().is_empty() {
        "AI agent".to_string()
    } else {
        clean
    }
}

fn known_name(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "claude-code" => "Claude Code",
        "claude-ai" | "claude" => "Claude",
        "cursor-vscode" | "cursor" => "Cursor",
        "codex" | "codex-mcp-client" | "codex_cli_rs" => "Codex",
        "gemini-cli-mcp-client" | "gemini-cli" => "Gemini CLI",
        "windsurf-client" | "windsurf" => "Windsurf",
        "zed" => "Zed",
        "mcp-inspector" | "inspector-client" => "MCP Inspector",
        _ => return None,
    })
}

/// The `_meta` of a request, for the bridge (which sends it on unchanged).
pub(super) fn meta_of(message: &Value) -> Option<&Map<String, Value>> {
    message.get("params")?.get("_meta")?.as_object()
}

/// The protocol version a request carries, for the bridge's headers.
pub(super) fn version_of(message: &Value) -> Option<&str> {
    meta_of(message)?.get(META_VERSION)?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(step: Step) -> Reply {
        match step {
            Step::Done(reply) => reply,
            Step::Call { .. } => panic!("a tool call"),
        }
    }

    fn meta(session: Option<&str>, protocol: Option<&str>) -> Meta {
        Meta {
            session: session.map(Into::into),
            protocol: protocol.map(Into::into),
        }
    }

    #[test]
    fn sessions_of_older_clients() {
        let agents = Agents::default();
        let init = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "clientInfo": { "name": "claude-code", "version": "2.0" } }
        });
        let reply = answer(prepare(&agents, &meta(None, None), init));
        assert_eq!(reply.status, StatusCode::OK);
        let session = reply.session.expect("a session");
        let body = reply.body.unwrap();
        assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(body["result"]["serverInfo"]["name"], "nexssh");
        assert!(
            body["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("list_servers")
        );
        assert_eq!(agents.status().agents[0].name, "Claude Code");

        // The notification that follows gets nothing back.
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let reply = answer(prepare(
            &agents,
            &meta(Some(&session), Some("2025-06-18")),
            note,
        ));
        assert_eq!(reply.status, StatusCode::ACCEPTED);
        assert!(reply.body.is_none());

        let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        let reply = answer(prepare(
            &agents,
            &meta(Some(&session), Some("2025-06-18")),
            list.clone(),
        ));
        let tools = reply.body.unwrap()["result"]["tools"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(tools, 8);

        // A session NexSSH does not know: the client must start over.
        let reply = answer(prepare(
            &agents,
            &meta(Some("stale"), Some("2025-06-18")),
            list,
        ));
        assert_eq!(reply.status, StatusCode::NOT_FOUND);
        assert_eq!(reply.body.unwrap()["error"]["code"], SESSION_NOT_FOUND);

        // A tool call goes to the tools, with the session's caller.
        let call = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "list_servers" } });
        match prepare(&agents, &meta(Some(&session), None), call) {
            Step::Call { caller, id, .. } => {
                assert_eq!(caller.name, "Claude Code");
                assert_eq!(caller.key, session);
                assert_eq!(id, json!(3));
            }
            Step::Done(reply) => panic!("answered without the tool: {reply:?}"),
        }
    }

    #[test]
    fn unknown_revisions_get_the_newest_with_sessions() {
        let agents = Agents::default();
        for asked in ["1999-01-01", "2026-07-28"] {
            let init = json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": asked, "clientInfo": { "name": "x" } }
            });
            let body = answer(prepare(&agents, &meta(None, None), init))
                .body
                .unwrap();
            assert_eq!(
                body["result"]["protocolVersion"], LATEST_WITH_SESSIONS,
                "{asked}"
            );
        }
    }

    #[test]
    fn stateless_clients() {
        let agents = Agents::default();
        let request = |method: &str, version: &str| {
            json!({
                "jsonrpc": "2.0", "id": "a", "method": method,
                "params": { "name": "list_servers", "_meta": {
                    "io.modelcontextprotocol/protocolVersion": version,
                    "io.modelcontextprotocol/clientInfo": { "name": "codex", "version": "1" }
                } }
            })
        };
        let reply = answer(prepare(
            &agents,
            &meta(None, Some("2026-07-28")),
            request("server/discover", "2026-07-28"),
        ));
        let body = reply.body.unwrap();
        assert_eq!(body["result"]["supportedVersions"][0], "2026-07-28");
        assert_eq!(body["result"]["resultType"], "complete");
        assert!(reply.session.is_none(), "no sessions in 2026-07-28");

        // A session header from an older client is ignored, not refused.
        match prepare(
            &agents,
            &meta(Some("old"), Some("2026-07-28")),
            request("tools/call", "2026-07-28"),
        ) {
            Step::Call { caller, .. } => assert_eq!(caller.name, "Codex"),
            Step::Done(reply) => panic!("{reply:?}"),
        }

        let reply = answer(prepare(
            &agents,
            &meta(None, Some("2025-11-25")),
            request("tools/list", "2026-07-28"),
        ));
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert_eq!(reply.body.unwrap()["error"]["code"], HEADER_MISMATCH);

        let reply = answer(prepare(
            &agents,
            &meta(None, Some("2099-01-01")),
            request("tools/list", "2099-01-01"),
        ));
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        let body = reply.body.unwrap();
        assert_eq!(body["error"]["code"], UNSUPPORTED_VERSION);
        assert_eq!(body["error"]["data"]["requested"], "2099-01-01");

        let reply = answer(prepare(
            &agents,
            &meta(None, Some("2026-07-28")),
            request("prompts/list", "2026-07-28"),
        ));
        assert_eq!(reply.status, StatusCode::NOT_FOUND);
        assert_eq!(reply.body.unwrap()["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn odd_messages() {
        let agents = Agents::default();
        let reply = answer(prepare(&agents, &meta(None, None), json!("hello")));
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        let reply = answer(prepare(
            &agents,
            &meta(None, None),
            json!({ "jsonrpc": "2.0", "id": 5, "result": {} }),
        ));
        assert_eq!(reply.status, StatusCode::ACCEPTED, "a response");
        let reply = answer(prepare(
            &agents,
            &meta(None, None),
            json!({ "jsonrpc": "2.0", "id": 6, "method": "ping" }),
        ));
        assert_eq!(reply.body.unwrap()["result"]["resultType"], "complete");
        let reply = answer(prepare(
            &agents,
            &meta(None, None),
            json!({ "jsonrpc": "2.0", "id": 7, "method": "nope" }),
        ));
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "older clients get errors with 200"
        );
    }

    #[test]
    fn names_of_clients() {
        assert_eq!(
            client_name(Some(&json!({ "name": "claude-code" }))),
            "Claude Code"
        );
        assert_eq!(
            client_name(Some(&json!({ "name": "my-bot", "title": "My Bot" }))),
            "My Bot"
        );
        assert_eq!(client_name(Some(&json!({ "name": "my-bot" }))), "my-bot");
        assert_eq!(client_name(Some(&json!({ "name": "  " }))), "AI agent");
        assert_eq!(client_name(None), "AI agent");
        assert_eq!(
            client_name(Some(&json!({ "name": "a\u{1b}[31mb" }))),
            "a[31mb"
        );
    }
}
