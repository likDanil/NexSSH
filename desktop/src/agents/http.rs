//! MCP's Streamable HTTP transport, as far as NexSSH needs it: one endpoint, `/mcp`, on
//! `127.0.0.1`; a POST carries one JSON-RPC message (or a batch, from older clients) and its
//! answer comes back as JSON. NexSSH never sends requests to agents, so there is no event
//! stream: GET is refused, as the protocol allows.
//!
//! Every request needs the bearer token. The `Host` and `Origin` headers must name this
//! computer too, so a web page cannot reach the server through DNS rebinding even before
//! the token is checked.

use std::convert::Infallible;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use super::{Agents, rpc};

/// The largest request: a file written whole comes in one message.
const MAX_BODY: usize = 24 * 1024 * 1024;

pub(super) const SESSION_HEADER: &str = "mcp-session-id";

/// Accepts connections until the task is aborted.
pub(super) async fn serve(listener: tokio::net::TcpListener, app: AppHandle) {
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(e) => {
                // Out of descriptors and the like: wait a little rather than spin.
                log::warn!("agents: accept failed: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let service = hyper::service::service_fn(move |request| {
                let app = app.clone();
                async move { Ok::<_, Infallible>(handle(&app, request).await) }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

async fn handle(app: &AppHandle, request: Request<Incoming>) -> Response<Full<Bytes>> {
    if request.uri().path().trim_end_matches('/') != "/mcp" {
        return plain(StatusCode::NOT_FOUND, "not found");
    }
    let headers = request.headers();
    if !local_host(headers) || !local_origin(headers) {
        return plain(StatusCode::FORBIDDEN, "forbidden");
    }
    let agents = app.state::<Agents>();
    if !agents.authorized(bearer(headers)) {
        let mut response = plain(
            StatusCode::UNAUTHORIZED,
            "the NexSSH token is missing or wrong",
        );
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"NexSSH\""),
        );
        return response;
    }
    match *request.method() {
        Method::POST => {}
        Method::DELETE => {
            // The end of an MCP session (revisions before 2026-07-28).
            return match header_text(headers, SESSION_HEADER) {
                Some(session) => {
                    agents.end_client(session);
                    empty(StatusCode::NO_CONTENT)
                }
                None => not_allowed(),
            };
        }
        _ => return not_allowed(),
    }

    let headers = request.headers().clone();
    let body = match Limited::new(request.into_body(), MAX_BODY).collect().await {
        Ok(body) => body.to_bytes(),
        Err(_) => return plain(StatusCode::PAYLOAD_TOO_LARGE, "the message is too large"),
    };
    let message: Value = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(_) => {
            let error = rpc::error_message(Value::Null, rpc::PARSE_ERROR, "Parse error", None);
            return json_response(StatusCode::BAD_REQUEST, &error, None);
        }
    };
    let reply = rpc::handle(app, &Meta::from_headers(&headers), message).await;
    match reply.body {
        Some(body) => json_response(reply.status, &body, reply.session.as_deref()),
        None => empty(reply.status),
    }
}

/// What the headers of a POST say about the message.
#[derive(Debug, Default, Clone)]
pub(super) struct Meta {
    pub session: Option<String>,
    pub protocol: Option<String>,
}

impl Meta {
    fn from_headers(headers: &HeaderMap) -> Meta {
        Meta {
            session: header_text(headers, SESSION_HEADER).map(str::to_string),
            protocol: header_text(headers, "mcp-protocol-version").map(str::to_string),
        }
    }
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = header_text(headers, header::AUTHORIZATION.as_str())?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

/// `localhost`, `127.0.0.1` or `[::1]`, with or without a port.
fn is_local(host_port: &str) -> bool {
    let host = if let Some(rest) = host_port.strip_prefix('[') {
        rest.split_once(']').map_or(rest, |(host, _)| host)
    } else {
        host_port
            .rsplit_once(':')
            .map_or(host_port, |(host, _)| host)
    };
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

fn local_host(headers: &HeaderMap) -> bool {
    header_text(headers, header::HOST.as_str()).is_none_or(is_local)
}

/// Agents send no `Origin`; a browser always does, and only a page of this computer passes.
fn local_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = header_text(headers, header::ORIGIN.as_str()) else {
        return true;
    };
    origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .is_some_and(is_local)
}

fn plain(status: StatusCode, text: &'static str) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from_static(text.as_bytes())));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

fn empty(status: StatusCode) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = status;
    response
}

fn not_allowed() -> Response<Full<Bytes>> {
    let mut response = plain(StatusCode::METHOD_NOT_ALLOWED, "only POST");
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("POST, DELETE"));
    response
}

fn json_response(status: StatusCode, body: &Value, session: Option<&str>) -> Response<Full<Bytes>> {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| json!({}).to_string().into_bytes());
    let mut response = Response::new(Full::new(Bytes::from(bytes)));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if let Some(value) = session.and_then(|s| HeaderValue::from_str(s).ok()) {
        headers.insert(SESSION_HEADER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn only_this_computer() {
        for host in [
            "127.0.0.1:7422",
            "localhost:7422",
            "LOCALHOST",
            "[::1]:7422",
            "127.0.0.1",
        ] {
            assert!(local_host(&headers(&[("host", host)])), "{host}");
        }
        for host in [
            "evil.example:7422",
            "127.0.0.1.evil.example",
            "[::2]:1",
            "192.168.1.5:7422",
        ] {
            assert!(!local_host(&headers(&[("host", host)])), "{host}");
        }
        assert!(
            local_host(&headers(&[])),
            "HTTP/1.0 clients may send no Host"
        );

        assert!(local_origin(&headers(&[])), "agents send no Origin");
        assert!(local_origin(&headers(&[(
            "origin",
            "http://localhost:3000"
        )])));
        assert!(local_origin(&headers(&[("origin", "http://127.0.0.1")])));
        for origin in [
            "https://evil.example",
            "null",
            "file://",
            "http://localhost.evil.example",
        ] {
            assert!(!local_origin(&headers(&[("origin", origin)])), "{origin}");
        }
    }

    #[test]
    fn reads_the_bearer_token() {
        assert_eq!(
            bearer(&headers(&[("authorization", "Bearer abc")])),
            Some("abc")
        );
        assert_eq!(
            bearer(&headers(&[("authorization", "bearer  abc ")])),
            Some("abc")
        );
        assert_eq!(bearer(&headers(&[("authorization", "Basic abc")])), None);
        assert_eq!(bearer(&headers(&[])), None);
    }
}
