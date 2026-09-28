//! Port forwarding over an established connection: local (`-L`), remote (`-R`) and
//! dynamic SOCKS5 (`-D`).

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use russh::Channel;
use russh::client::{Handle, Msg};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::error::{Error, Result};
use crate::i18n;
use crate::model::{ForwardKind, ForwardSpec};
use crate::session::handler::{ClientHandler, ConnState};
use crate::session::{LogLevel, SessionCtx, SessionEvent};
use crate::util;

/// An active forward as reported to the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardInfo {
    pub id: u64,
    pub spec: ForwardSpec,
    pub description: String,
    /// Started from the server's saved configuration.
    pub saved: bool,
}

struct Active {
    id: u64,
    spec: ForwardSpec,
    saved: bool,
    listener: Option<JoinHandle<()>>,
    remote: Option<(String, u32)>,
}

/// Forwards of one connection. Dropping it stops the local listeners.
pub(crate) struct Forwards {
    ctx: Arc<SessionCtx>,
    handle: Arc<Handle<ClientHandler>>,
    state: Arc<ConnState>,
    active: Vec<Active>,
    next_id: u64,
}

impl Forwards {
    pub fn new(
        ctx: Arc<SessionCtx>,
        handle: Arc<Handle<ClientHandler>>,
        state: Arc<ConnState>,
    ) -> Self {
        Forwards {
            ctx,
            handle,
            state,
            active: Vec::new(),
            next_id: 1,
        }
    }

    pub async fn start(&mut self, mut spec: ForwardSpec, saved: bool) -> Result<()> {
        spec.normalize()?;
        let id = self.next_id;
        self.next_id += 1;
        let bind = util::host_port(&spec.bind_host, spec.bind_port);
        let active = match spec.kind {
            ForwardKind::Local | ForwardKind::Dynamic => {
                let listener = TcpListener::bind((spec.bind_host.as_str(), spec.bind_port))
                    .await
                    .map_err(|e| Error::invalid(i18n::cannot_listen(&bind, e)))?;
                let task = tokio::spawn(accept_loop(
                    listener,
                    Arc::clone(&self.handle),
                    spec.clone(),
                ));
                Active {
                    id,
                    spec,
                    saved,
                    listener: Some(task),
                    remote: None,
                }
            }
            ForwardKind::Remote => {
                let port = self
                    .handle
                    .tcpip_forward(spec.bind_host.clone(), u32::from(spec.bind_port))
                    .await
                    .map_err(|e| Error::invalid(i18n::server_refused_listen(&bind, e)))?;
                let bound = if spec.bind_port == 0 {
                    port
                } else {
                    u32::from(spec.bind_port)
                };
                if spec.bind_port == 0 {
                    spec.bind_port = u16::try_from(bound).unwrap_or(0);
                }
                self.state.add_remote_forward(
                    spec.bind_host.clone(),
                    bound,
                    (spec.target_host.clone(), spec.target_port),
                );
                Active {
                    id,
                    remote: Some((spec.bind_host.clone(), bound)),
                    spec,
                    saved,
                    listener: None,
                }
            }
        };
        self.ctx
            .log(LogLevel::Info, i18n::forwarding(&active.spec.describe()));
        self.active.push(active);
        Ok(())
    }

    pub async fn stop(&mut self, id: u64) {
        if let Some(pos) = self.active.iter().position(|a| a.id == id) {
            let active = self.active.remove(pos);
            self.shutdown(active).await;
        }
    }

    pub async fn stop_all(&mut self) {
        for active in std::mem::take(&mut self.active) {
            self.shutdown(active).await;
        }
    }

    async fn shutdown(&self, active: Active) {
        if let Some(task) = active.listener {
            task.abort();
        }
        if let Some((address, port)) = active.remote {
            self.state.remove_remote_forward(&address, port);
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                self.handle.cancel_tcpip_forward(address, port),
            )
            .await;
        }
    }

    pub fn list(&self) -> Vec<ForwardInfo> {
        self.active
            .iter()
            .map(|a| ForwardInfo {
                id: a.id,
                spec: a.spec.clone(),
                description: a.spec.describe(),
                saved: a.saved,
            })
            .collect()
    }

    pub fn publish(&self) {
        self.ctx.event(SessionEvent::Forwards {
            forwards: self.list(),
        });
    }
}

impl Drop for Forwards {
    fn drop(&mut self) {
        for active in &self.active {
            if let Some(task) = &active.listener {
                task.abort();
            }
        }
    }
}

async fn accept_loop(listener: TcpListener, handle: Arc<Handle<ClientHandler>>, spec: ForwardSpec) {
    loop {
        let (socket, peer) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                log::warn!("forward accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        let handle = Arc::clone(&handle);
        let spec = spec.clone();
        tokio::spawn(async move {
            let _ = socket.set_nodelay(true);
            let result = match spec.kind {
                ForwardKind::Dynamic => socks5(&handle, socket, peer).await,
                _ => tunnel(&handle, socket, peer, &spec.target_host, spec.target_port).await,
            };
            if let Err(e) = result {
                log::debug!("forwarded connection from {peer} ended: {e}");
            }
        });
    }
}

async fn open_channel(
    handle: &Handle<ClientHandler>,
    host: &str,
    port: u16,
    peer: SocketAddr,
) -> Result<Channel<Msg>> {
    Ok(handle
        .channel_open_direct_tcpip(
            host,
            u32::from(port),
            peer.ip().to_string(),
            u32::from(peer.port()),
        )
        .await?)
}

async fn tunnel(
    handle: &Handle<ClientHandler>,
    mut socket: TcpStream,
    peer: SocketAddr,
    host: &str,
    port: u16,
) -> Result<()> {
    let channel = open_channel(handle, host, port, peer).await?;
    let mut stream = channel.into_stream();
    tokio::io::copy_bidirectional(&mut socket, &mut stream).await?;
    Ok(())
}

/// Connects a channel opened by the server (remote forward) to a local target.
pub(crate) async fn pipe_to_local(channel: Channel<Msg>, host: String, port: u16) {
    match TcpStream::connect((host.as_str(), port)).await {
        Ok(mut socket) => {
            let _ = socket.set_nodelay(true);
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
        }
        Err(e) => {
            log::warn!(
                "remote forward: cannot connect to {}: {e}",
                util::host_port(&host, port)
            );
            let _ = channel.close().await;
        }
    }
}

/// Minimal SOCKS5 server (RFC 1928): no authentication, CONNECT only.
async fn socks5(
    handle: &Handle<ClientHandler>,
    mut socket: TcpStream,
    peer: SocketAddr,
) -> Result<()> {
    let mut head = [0u8; 2];
    socket.read_exact(&mut head).await?;
    if head[0] != 5 {
        return Err(Error::invalid("not a SOCKS5 client"));
    }
    let mut methods = vec![0u8; usize::from(head[1])];
    socket.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        socket.write_all(&[5, 0xff]).await?;
        return Err(Error::invalid("SOCKS client requires authentication"));
    }
    socket.write_all(&[5, 0]).await?;

    let mut request = [0u8; 4];
    socket.read_exact(&mut request).await?;
    if request[1] != 1 {
        socket.write_all(&socks_reply(7)).await?;
        return Err(Error::invalid("only SOCKS CONNECT is supported"));
    }
    let host = match request[3] {
        1 => {
            let mut a = [0u8; 4];
            socket.read_exact(&mut a).await?;
            Ipv4Addr::from(a).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            socket.read_exact(&mut len).await?;
            let mut name = vec![0u8; usize::from(len[0])];
            socket.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(|_| Error::invalid("invalid host name"))?
        }
        4 => {
            let mut a = [0u8; 16];
            socket.read_exact(&mut a).await?;
            Ipv6Addr::from(a).to_string()
        }
        _ => {
            socket.write_all(&socks_reply(8)).await?;
            return Err(Error::invalid("unsupported SOCKS address type"));
        }
    };
    let mut port = [0u8; 2];
    socket.read_exact(&mut port).await?;
    let port = u16::from_be_bytes(port);

    match open_channel(handle, &host, port, peer).await {
        Ok(channel) => {
            socket.write_all(&socks_reply(0)).await?;
            let mut stream = channel.into_stream();
            tokio::io::copy_bidirectional(&mut socket, &mut stream).await?;
            Ok(())
        }
        Err(e) => {
            socket.write_all(&socks_reply(5)).await?;
            Err(e)
        }
    }
}

fn socks_reply(code: u8) -> [u8; 10] {
    [5, code, 0, 1, 0, 0, 0, 0, 0, 0]
}
