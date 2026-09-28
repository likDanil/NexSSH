//! russh client callbacks: host key verification, banners, remote forwarding.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use russh::Channel;
use russh::client::{self, ChannelOpenHandle, DisconnectReason, Msg, Session};
use russh::keys::{HashAlg, PublicKey, PublicKeyOrCertificate};

use super::{LogLevel, Prompt, PromptReply, SessionCtx, lock};
use crate::error::Error;
use crate::known_hosts::HostKeyStatus;

/// State of one SSH connection shared between the handler (running inside russh's
/// task) and the session/forwarding code.
#[derive(Default)]
pub(crate) struct ConnState {
    reason: Mutex<Option<String>>,
    /// Remote forwards: (bound address, bound port) → local target.
    remote_forwards: Mutex<HashMap<(String, u32), (String, u16)>>,
}

impl ConnState {
    pub fn set_reason(&self, reason: String) {
        let mut slot = lock(&self.reason);
        if slot.is_none() {
            *slot = Some(reason);
        }
    }

    pub fn reason(&self) -> Option<String> {
        lock(&self.reason).clone()
    }

    pub fn add_remote_forward(&self, address: String, port: u32, target: (String, u16)) {
        lock(&self.remote_forwards).insert((address, port), target);
    }

    pub fn remove_remote_forward(&self, address: &str, port: u32) {
        lock(&self.remote_forwards).remove(&(address.to_string(), port));
    }

    /// Servers may echo a different spelling of the bind address (e.g. "" vs
    /// "localhost"), so fall back to matching on the port alone.
    fn remote_target(&self, address: &str, port: u32) -> Option<(String, u16)> {
        let map = lock(&self.remote_forwards);
        map.get(&(address.to_string(), port)).cloned().or_else(|| {
            let mut by_port = map.iter().filter(|((_, p), _)| *p == port);
            match (by_port.next(), by_port.next()) {
                (Some((_, target)), None) => Some(target.clone()),
                _ => None,
            }
        })
    }
}

pub(crate) struct ClientHandler {
    ctx: Arc<SessionCtx>,
    host: String,
    port: u16,
    state: Arc<ConnState>,
}

impl ClientHandler {
    pub fn new(ctx: Arc<SessionCtx>, host: String, port: u16, state: Arc<ConnState>) -> Self {
        ClientHandler {
            ctx,
            host,
            port,
            state,
        }
    }
}

impl client::Handler for ClientHandler {
    type Error = Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key: PublicKey = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            // Host certificates are not requested, but handle them by their key (TOFU).
            PublicKeyOrCertificate::Certificate(cert) => PublicKey::from(cert.public_key().clone()),
        };
        let known_hosts = &self.ctx.shared.known_hosts;
        let check = known_hosts.check(&self.host, self.port, &key);
        match check {
            HostKeyStatus::Trusted => Ok(true),
            HostKeyStatus::Revoked => Err(Error::HostKey(format!(
                "the key presented by {} is marked as revoked",
                self.host
            ))),
            HostKeyStatus::Unknown | HostKeyStatus::Changed { .. } => {
                let changed = matches!(check, HostKeyStatus::Changed { .. });
                let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
                let reply = self
                    .ctx
                    .ask(Prompt::HostKey {
                        host: self.host.clone(),
                        port: self.port,
                        key_type: key.algorithm().to_string(),
                        fingerprint: fingerprint.clone(),
                        check,
                    })
                    .await;
                match reply {
                    Some(PromptReply::HostKey {
                        accept: true,
                        remember,
                    }) => {
                        if remember && let Err(e) = known_hosts.learn(&self.host, self.port, &key) {
                            self.ctx
                                .log(LogLevel::Warn, format!("Could not save host key: {e}"));
                        }
                        if changed {
                            self.ctx.log(
                                LogLevel::Warn,
                                format!(
                                    "Accepted a changed host key for {} ({fingerprint})",
                                    self.host
                                ),
                            );
                        }
                        Ok(true)
                    }
                    _ => Err(Error::HostKey(if changed {
                        "the host key has changed and was not accepted".into()
                    } else {
                        "the host key was not accepted".into()
                    })),
                }
            }
        }
    }

    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let text = banner.replace("\r\n", "\n").replace('\n', "\r\n");
        self.ctx.sink.output(text.into_bytes());
        Ok(())
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        match self.state.remote_target(connected_address, connected_port) {
            Some((host, port)) => {
                reply.accept().await;
                tokio::spawn(crate::forward::pipe_to_local(channel, host, port));
            }
            None => {
                log::warn!(
                    "unexpected forwarded connection for {connected_address}:{connected_port}"
                );
                drop(reply);
            }
        }
        Ok(())
    }

    async fn disconnected(
        &mut self,
        reason: DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        match reason {
            DisconnectReason::ReceivedDisconnect(info) => {
                let msg = if info.message.trim().is_empty() {
                    format!("closed by server ({:?})", info.reason_code)
                } else {
                    format!("closed by server: {}", info.message.trim())
                };
                self.state.set_reason(msg);
                Ok(())
            }
            DisconnectReason::Error(e) => {
                self.state.set_reason(e.to_string());
                Err(e)
            }
        }
    }
}
