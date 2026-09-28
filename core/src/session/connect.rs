//! Establishing connections: DNS/TCP with timeouts, jump host chains, handshake and auth.

use std::borrow::Cow;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::Algorithm;
use russh::{Disconnect, Preferred, SshId};
use tokio::net::TcpStream;

use super::handler::{ClientHandler, ConnState};
use super::{LogLevel, SessionCtx, auth};
use crate::error::{Error, Result};
use crate::known_hosts::preferred_host_key_algorithms;
use crate::model::{Destination, Server};
use crate::store::ServerStore;
use crate::util;

/// Longest supported jump host chain (including the target).
const MAX_HOPS: usize = 8;

/// An authenticated connection to the target, plus the jump host connections it runs through.
pub(crate) struct Connection {
    pub handle: Arc<Handle<ClientHandler>>,
    pub jumps: Vec<Arc<Handle<ClientHandler>>>,
    pub state: Arc<ConnState>,
}

impl Connection {
    pub async fn disconnect(&self) {
        let _ = self
            .handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
        for jump in self.jumps.iter().rev() {
            let _ = jump.disconnect(Disconnect::ByApplication, "", "en").await;
        }
    }
}

/// Expands jump hosts (recursively, like chained `ProxyJump`s) into connection order,
/// ending with `target`.
pub(crate) fn resolve_chain(store: &ServerStore, target: &Server) -> Result<Vec<Server>> {
    let mut chain = Vec::new();
    if let Some(spec) = &target.jump_host {
        expand_jumps(store, spec, &mut chain, 0)?;
    }
    chain.push(target.clone());
    if chain.len() > MAX_HOPS {
        return Err(Error::invalid("the jump host chain is too long"));
    }
    Ok(chain)
}

fn expand_jumps(
    store: &ServerStore,
    spec: &str,
    out: &mut Vec<Server>,
    depth: usize,
) -> Result<()> {
    if depth >= MAX_HOPS {
        return Err(Error::invalid(
            "the jump host chain is too long (do two servers use each other as jump host?)",
        ));
    }
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if part.eq_ignore_ascii_case("none") {
            continue;
        }
        match store.find(part) {
            Some(server) => {
                if let Some(inner) = &server.jump_host {
                    expand_jumps(store, inner, out, depth + 1)?;
                }
                out.push(server);
            }
            None => out.push(Destination::parse(part)?.to_server()),
        }
        if out.len() >= MAX_HOPS {
            return Err(Error::invalid("the jump host chain is too long"));
        }
    }
    Ok(())
}

pub(crate) async fn establish(ctx: &Arc<SessionCtx>, target: &Server) -> Result<Connection> {
    let chain = resolve_chain(&ctx.shared.store, target)?;
    let state = Arc::new(ConnState::default());
    let mut handles: Vec<Arc<Handle<ClientHandler>>> = Vec::with_capacity(chain.len());
    for (i, hop) in chain.iter().enumerate() {
        let is_target = i + 1 == chain.len();
        let message = match (is_target, i) {
            (true, 0) => format!("Connecting to {}…", hop.destination()),
            (true, _) => format!("Connecting to {} through the jump host…", hop.destination()),
            (false, _) => format!("Connecting to jump host {}…", hop.destination()),
        };
        ctx.log(LogLevel::Info, message);
        let via = handles.last().cloned();
        let secs = hop.connect_timeout();
        let result = with_deadline(ctx, secs, connect_hop(ctx, hop, via, &state))
            .await
            .and_then(|r| r);
        let handle = match result {
            Ok(h) => h,
            Err(e) => {
                for h in handles.iter().rev() {
                    let _ = h.disconnect(Disconnect::ByApplication, "", "en").await;
                }
                return Err(if is_target {
                    e
                } else {
                    Error::Disconnected(format!("jump host {}: {e}", hop.destination()))
                });
            }
        };
        handles.push(Arc::new(handle));
    }
    let handle = handles.pop().expect("the chain always contains the target");
    Ok(Connection {
        handle,
        jumps: handles,
        state,
    })
}

async fn connect_hop(
    ctx: &Arc<SessionCtx>,
    hop: &Server,
    via: Option<Arc<Handle<ClientHandler>>>,
    state: &Arc<ConnState>,
) -> Result<Handle<ClientHandler>> {
    let known = ctx.shared.known_hosts.known_algorithms(&hop.host, hop.port);
    let config = client_config(hop, &known);
    let handler = ClientHandler::new(
        Arc::clone(ctx),
        hop.host.clone(),
        hop.port,
        Arc::clone(state),
    );
    let mut handle = match via {
        None => {
            let stream = tcp_connect(&hop.host, hop.port).await?;
            client::connect_stream(config, stream, handler).await?
        }
        Some(jump) => {
            let channel = jump
                .channel_open_direct_tcpip(hop.host.clone(), u32::from(hop.port), "127.0.0.1", 0)
                .await
                .map_err(|e| {
                    Error::Disconnected(format!(
                        "the jump host could not reach {}: {e}",
                        util::host_port(&hop.host, hop.port)
                    ))
                })?;
            client::connect_stream(config, channel.into_stream(), handler).await?
        }
    };
    auth::authenticate(&mut handle, hop, ctx).await?;
    Ok(handle)
}

fn client_config(hop: &Server, known: &[Algorithm]) -> Arc<client::Config> {
    let preferred = Preferred {
        key: Cow::Owned(preferred_host_key_algorithms(known)),
        ..Preferred::default()
    };
    Arc::new(client::Config {
        client_id: SshId::Standard(Cow::Owned(format!(
            "SSH-2.0-NexSSH_{}",
            env!("CARGO_PKG_VERSION")
        ))),
        keepalive_interval: hop.keepalive(),
        keepalive_max: 3,
        inactivity_timeout: None,
        nodelay: true,
        preferred,
        ..Default::default()
    })
}

async fn tcp_connect(host: &str, port: u16) -> Result<TcpStream> {
    let target = util::host_port(host, port);
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| Error::Disconnected(format!("cannot resolve {host}: {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(Error::Disconnected(format!("cannot resolve {host}")));
    }
    let mut last_err = None;
    let count = addrs.len();
    for (i, addr) in addrs.into_iter().enumerate() {
        // With several addresses (IPv6 + IPv4) do not let one black-holed address eat
        // the whole connection timeout.
        let attempt = TcpStream::connect(addr);
        let result = if i + 1 < count {
            match tokio::time::timeout(Duration::from_secs(4), attempt).await {
                Ok(r) => r,
                Err(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timed out",
                )),
            }
        } else {
            attempt.await
        };
        match result {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                return Ok(stream);
            }
            Err(e) => last_err = Some(e),
        }
    }
    let err = last_err.map(|e| e.to_string()).unwrap_or_default();
    Err(Error::Disconnected(format!(
        "cannot connect to {target}: {err}"
    )))
}

/// Like `tokio::time::timeout`, but time the user spends answering prompts (host key,
/// password, 2FA) does not count.
async fn with_deadline<F: Future>(ctx: &SessionCtx, secs: u32, fut: F) -> Result<F::Output> {
    tokio::pin!(fut);
    let period = Duration::from_secs(u64::from(secs));
    ctx.take_prompted();
    loop {
        tokio::select! {
            out = &mut fut => return Ok(out),
            _ = tokio::time::sleep(period) => {
                if ctx.take_prompted() {
                    continue;
                }
                return Err(Error::Timeout(secs));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(servers: &[Server]) -> ServerStore {
        let dir = std::env::temp_dir().join(format!("nexssh-chain-{}", util::random_id()));
        let store = ServerStore::open(dir.join("servers.json")).unwrap();
        for s in servers {
            store.save_server(s.clone()).unwrap();
        }
        store
    }

    fn named(name: &str, jump: Option<&str>) -> Server {
        Server {
            name: name.into(),
            host: format!("{name}.example"),
            jump_host: jump.map(Into::into),
            ..Server::default()
        }
    }

    #[test]
    fn resolves_nested_jump_hosts() {
        let store = store_with(&[named("edge", None), named("bastion", Some("edge"))]);
        let target = named("db", Some("bastion"));
        let chain = resolve_chain(&store, &target).unwrap();
        let hosts: Vec<&str> = chain.iter().map(|s| s.host.as_str()).collect();
        assert_eq!(hosts, ["edge.example", "bastion.example", "db.example"]);
    }

    #[test]
    fn resolves_literal_and_comma_separated_jumps() {
        let store = store_with(&[]);
        let target = named("db", Some("ops@hop1:2222, hop2"));
        let chain = resolve_chain(&store, &target).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].user, "ops");
        assert_eq!(chain[0].port, 2222);
        assert_eq!(chain[1].host, "hop2");
    }

    #[test]
    fn detects_jump_loops() {
        let store = store_with(&[named("a", Some("b")), named("b", Some("a"))]);
        let target = named("x", Some("a"));
        assert!(resolve_chain(&store, &target).is_err());
    }
}
