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
use crate::i18n;
use crate::known_hosts::preferred_host_key_algorithms;
use crate::model::{Destination, Server};
use crate::secrets::Secrets;
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

/// A server on the way to the target (or the target itself).
#[derive(Debug)]
pub(crate) struct Hop {
    pub server: Server,
    /// Keychain account of its password. `None` for hosts that are not saved anywhere:
    /// their password is asked for and cannot be remembered.
    pub password_account: Option<String>,
}

/// Expands jump hosts (recursively, like chained `ProxyJump`s) into connection order,
/// ending with `target`.
pub(crate) fn resolve_chain(store: &ServerStore, target: &Server) -> Result<Vec<Hop>> {
    let mut chain = Vec::new();
    expand_jumps(store, target, &mut chain, 0)?;
    let saved = is_saved(store, target);
    chain.push(Hop {
        server: target.clone(),
        password_account: saved.then(|| Secrets::password_account(&target.id)),
    });
    if chain.len() > MAX_HOPS {
        return Err(Error::invalid(i18n::jump_chain_too_long()));
    }
    Ok(chain)
}

fn is_saved(store: &ServerStore, server: &Server) -> bool {
    !server.id.is_empty() && store.get(&server.id).is_some()
}

/// Appends the jump hosts of `owner`. A jump host typed in place (`host[:port]`, not a
/// saved server) signs in with the owner's jump host login and remembered password, when
/// it is the only one: in a chain the hops would all share them.
fn expand_jumps(
    store: &ServerStore,
    owner: &Server,
    out: &mut Vec<Hop>,
    depth: usize,
) -> Result<()> {
    let Some(spec) = &owner.jump_host else {
        return Ok(());
    };
    if depth >= MAX_HOPS {
        return Err(Error::invalid(i18n::jump_chain_loop()));
    }
    let parts: Vec<&str> = spec
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case("none"))
        .collect();
    let single = parts.len() == 1;
    for part in parts {
        match store.find(part) {
            Some(server) => {
                expand_jumps(store, &server, out, depth + 1)?;
                out.push(Hop {
                    password_account: Some(Secrets::password_account(&server.id)),
                    server,
                });
            }
            None => {
                let mut server = Destination::parse(part)?.to_server();
                let mut password_account = None;
                if single {
                    if let Some(user) = &owner.jump_user {
                        server.user = user.clone();
                    }
                    if is_saved(store, owner) {
                        password_account = Some(Secrets::jump_password_account(&owner.id));
                    }
                }
                out.push(Hop {
                    server,
                    password_account,
                });
            }
        }
        if out.len() >= MAX_HOPS {
            return Err(Error::invalid(i18n::jump_chain_too_long()));
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
        let dest = hop.server.destination();
        let message = match (is_target, i) {
            (true, 0) => i18n::connecting(&dest),
            (true, _) => i18n::connecting_through_jump(&dest),
            (false, _) => i18n::connecting_to_jump(&dest),
        };
        ctx.log(LogLevel::Info, message);
        let via = handles.last().cloned();
        let secs = hop.server.connect_timeout();
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
                    Error::Disconnected(i18n::jump_host_failed(&dest, e))
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
    target: &Hop,
    via: Option<Arc<Handle<ClientHandler>>>,
    state: &Arc<ConnState>,
) -> Result<Handle<ClientHandler>> {
    let hop = &target.server;
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
                    Error::Disconnected(i18n::jump_host_unreachable(
                        &util::host_port(&hop.host, hop.port),
                        e,
                    ))
                })?;
            client::connect_stream(config, channel.into_stream(), handler).await?
        }
    };
    auth::authenticate(&mut handle, hop, target.password_account.as_deref(), ctx).await?;
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
        .map_err(|e| Error::Disconnected(i18n::cannot_resolve_because(host, e)))?
        .collect();
    if addrs.is_empty() {
        return Err(Error::Disconnected(i18n::cannot_resolve(host)));
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
                    i18n::attempt_timed_out(),
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
    Err(Error::Disconnected(i18n::cannot_connect(&target, err)))
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
        let hosts: Vec<&str> = chain.iter().map(|h| h.server.host.as_str()).collect();
        assert_eq!(hosts, ["edge.example", "bastion.example", "db.example"]);
        // Saved jump hosts sign in with their own remembered passwords.
        let edge = store.find("edge").unwrap();
        assert_eq!(
            chain[0].password_account,
            Some(Secrets::password_account(&edge.id))
        );
        assert_eq!(chain[2].password_account, None, "the target is not saved");
    }

    #[test]
    fn resolves_literal_and_comma_separated_jumps() {
        let store = store_with(&[]);
        let mut target = named("db", Some("ops@hop1:2222, hop2"));
        target.jump_user = Some("ignored".into());
        let chain = resolve_chain(&store, &target).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].server.user, "ops");
        assert_eq!(chain[0].server.port, 2222);
        assert_eq!(chain[1].server.host, "hop2");
        // A chain typed in place has no login or password of its own.
        assert_eq!(chain[1].server.user, "");
        assert!(chain.iter().all(|h| h.password_account.is_none()));
    }

    #[test]
    fn jump_host_typed_in_place_uses_the_owners_login_and_password() {
        let mut db = named("db", Some("ops@bastion.example:2200"));
        db.jump_user = Some("admin".into());
        let store = store_with(&[db, named("app", Some("db"))]);
        let db = store.find("db").unwrap();

        let chain = resolve_chain(&store, &db).unwrap();
        let hop = &chain[0].server;
        assert_eq!((hop.user.as_str(), hop.port), ("admin", 2200));
        assert_eq!(
            chain[0].password_account,
            Some(Secrets::jump_password_account(&db.id))
        );
        assert_eq!(
            chain[1].password_account,
            Some(Secrets::password_account(&db.id))
        );

        // Through another server, db's jump host still uses db's login.
        let app = store.find("app").unwrap();
        let chain = resolve_chain(&store, &app).unwrap();
        let users: Vec<&str> = chain.iter().map(|h| h.server.user.as_str()).collect();
        assert_eq!(users, ["admin", "", ""]);
        assert_eq!(
            chain[0].password_account,
            Some(Secrets::jump_password_account(&db.id))
        );

        // Without a login of its own, the one typed with the host stays.
        let mut quick = named("quick", Some("ops@bastion.example"));
        quick.id = String::new();
        let chain = resolve_chain(&store, &quick).unwrap();
        assert_eq!(chain[0].server.user, "ops");
        assert_eq!(chain[0].password_account, None, "not a saved server");
    }

    #[test]
    fn detects_jump_loops() {
        let store = store_with(&[named("a", Some("b")), named("b", Some("a"))]);
        let target = named("x", Some("a"));
        assert!(resolve_chain(&store, &target).is_err());
    }
}
