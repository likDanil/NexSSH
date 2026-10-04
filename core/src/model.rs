//! Data model shared by storage, sessions and the UI (serialized as camelCase JSON).

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::{i18n, util};

pub const DEFAULT_PORT: u16 = 22;
pub const DEFAULT_KEEPALIVE_SECS: u32 = 30;
pub const DEFAULT_CONNECT_TIMEOUT_SECS: u32 = 15;

/// How a server authenticates the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    /// OpenSSH-like: agent, configured key, default keys, keyboard-interactive, password.
    #[default]
    Auto,
    /// Password (stored in the OS keychain or asked for on connect).
    Password,
    /// A specific private key file (passphrase stored in the keychain or asked for).
    Key,
    /// Only identities offered by the running SSH agent.
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForwardKind {
    /// `-L`: a local port tunnelled to `target` as seen from the server.
    Local,
    /// `-R`: a port on the server tunnelled back to `target` as seen from this machine.
    Remote,
    /// `-D`: a local SOCKS5 proxy that opens connections through the server.
    Dynamic,
}

/// A port forwarding rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardSpec {
    pub kind: ForwardKind,
    #[serde(default = "default_bind_host")]
    pub bind_host: String,
    pub bind_port: u16,
    #[serde(default)]
    pub target_host: String,
    #[serde(default)]
    pub target_port: u16,
}

fn default_bind_host() -> String {
    "127.0.0.1".into()
}

impl ForwardSpec {
    pub fn normalize(&mut self) -> Result<()> {
        self.bind_host = self.bind_host.trim().to_string();
        self.target_host = self.target_host.trim().to_string();
        if self.bind_host.is_empty() {
            self.bind_host = match self.kind {
                ForwardKind::Remote => "localhost".into(),
                _ => default_bind_host(),
            };
        }
        if self.bind_port == 0 && self.kind != ForwardKind::Remote {
            return Err(Error::invalid(i18n::forward_needs_local_port()));
        }
        if self.kind != ForwardKind::Dynamic {
            if self.target_host.is_empty() {
                return Err(Error::invalid(i18n::forward_needs_target_host()));
            }
            if self.target_port == 0 {
                return Err(Error::invalid(i18n::forward_needs_target_port()));
            }
        }
        Ok(())
    }

    /// Short human readable description, e.g. `localhost:8080 → db:5432`.
    pub fn describe(&self) -> String {
        let bind = util::host_port(&self.bind_host, self.bind_port);
        let target = util::host_port(&self.target_host, self.target_port);
        match self.kind {
            ForwardKind::Local => i18n::forward_local(&bind, &target),
            ForwardKind::Remote => i18n::forward_remote(&bind, &target),
            ForwardKind::Dynamic => i18n::forward_dynamic(&bind),
        }
    }
}

/// A saved server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    /// Remote user; empty means "same as the local user" (like OpenSSH).
    pub user: String,
    /// Group shown in the sidebar; empty means ungrouped.
    pub group: String,
    pub auth: AuthKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<String>,
    /// Jump host: the id or name of another saved server, or `[user@]host[:port]`.
    /// Several hops may be chained with commas, like OpenSSH's `ProxyJump`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jump_host: Option<String>,
    /// Login on the jump host when `jump_host` names one host that is not a saved server
    /// (a saved server signs in with its own settings). Its password, if remembered, is in
    /// the OS keychain under [`crate::secrets::Secrets::jump_password_account`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jump_user: Option<String>,
    /// Keepalive interval; `None` uses the default, `Some(0)` disables keepalives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keepalive_secs: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_timeout_secs: Option<u32>,
    /// Forwards started automatically after connecting.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<ForwardSpec>,
    /// `Host` alias when imported from `~/.ssh/config` (used to update on re-import).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Unix time of the last successful connection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<u64>,
}

impl Default for Server {
    fn default() -> Self {
        Server {
            id: String::new(),
            name: String::new(),
            host: String::new(),
            port: DEFAULT_PORT,
            user: String::new(),
            group: String::new(),
            auth: AuthKind::Auto,
            identity_file: None,
            jump_host: None,
            jump_user: None,
            keepalive_secs: None,
            connect_timeout_secs: None,
            forwards: Vec::new(),
            alias: None,
            last_used: None,
        }
    }
}

fn trim_opt(v: &mut Option<String>) {
    if let Some(s) = v {
        let t = s.trim();
        if t.is_empty() {
            *v = None;
        } else if t.len() != s.len() {
            *v = Some(t.to_string());
        }
    }
}

impl Server {
    /// Trims and validates user input. Called before a server is stored.
    pub fn normalize(&mut self) -> Result<()> {
        self.host = self.host.trim().to_string();
        self.user = self.user.trim().to_string();
        self.name = self.name.trim().to_string();
        self.group = self.group.trim().to_string();
        trim_opt(&mut self.identity_file);
        trim_opt(&mut self.jump_host);
        trim_opt(&mut self.jump_user);
        trim_opt(&mut self.alias);
        if self.jump_host.is_none() {
            self.jump_user = None;
        }

        if self.host.is_empty() {
            return Err(Error::invalid(i18n::host_required()));
        }
        if self.host.chars().any(char::is_whitespace) {
            return Err(Error::invalid(i18n::host_has_spaces()));
        }
        if self.port == 0 {
            return Err(Error::invalid(i18n::port_out_of_range()));
        }
        if self.user.chars().any(char::is_whitespace)
            || self
                .jump_user
                .as_deref()
                .is_some_and(|u| u.chars().any(char::is_whitespace))
        {
            return Err(Error::invalid(i18n::user_has_spaces()));
        }
        if self.name.is_empty() {
            self.name = self.host.clone();
        }
        if self.auth == AuthKind::Key && self.identity_file.is_none() {
            return Err(Error::invalid(i18n::key_file_required()));
        }
        if let Some(t) = self.connect_timeout_secs {
            self.connect_timeout_secs = Some(t.clamp(1, 600));
        }
        for fwd in &mut self.forwards {
            fwd.normalize()?;
        }
        Ok(())
    }

    /// `user@host[:port]` (port omitted when 22).
    pub fn destination(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let base = if self.user.is_empty() {
            host
        } else {
            format!("{}@{host}", self.user)
        };
        if self.port == DEFAULT_PORT {
            base
        } else {
            format!("{base}:{}", self.port)
        }
    }

    pub fn keepalive(&self) -> Option<std::time::Duration> {
        match self.keepalive_secs.unwrap_or(DEFAULT_KEEPALIVE_SECS) {
            0 => None,
            s => Some(std::time::Duration::from_secs(s as u64)),
        }
    }

    pub fn connect_timeout(&self) -> u32 {
        self.connect_timeout_secs
            .unwrap_or(DEFAULT_CONNECT_TIMEOUT_SECS)
            .clamp(1, 600)
    }
}

/// A destination typed by the user: `[ssh://][user@]host[:port]`, IPv6 allowed as `[addr]:port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl Destination {
    pub fn parse(input: &str) -> Result<Destination> {
        let s = input.trim();
        let s = s.strip_prefix("ssh://").unwrap_or(s).trim_end_matches('/');
        if s.is_empty() {
            return Err(Error::invalid(i18n::destination_required()));
        }
        if s.chars().any(char::is_whitespace) {
            return Err(Error::invalid(i18n::destination_has_spaces()));
        }
        let (user, hostport) = match s.rsplit_once('@') {
            Some((u, h)) if !u.is_empty() => (Some(u.to_string()), h),
            Some((_, h)) => (None, h),
            None => (None, s),
        };
        let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
            let (h, after) = rest
                .split_once(']')
                .ok_or_else(|| Error::invalid(i18n::ipv6_missing_bracket()))?;
            let port = match after.strip_prefix(':') {
                Some(p) => Some(parse_port(p)?),
                None if after.is_empty() => None,
                None => return Err(Error::invalid(i18n::ipv6_trailing_text())),
            };
            (h.to_string(), port)
        } else if hostport.matches(':').count() == 1 {
            let (h, p) = hostport.split_once(':').unwrap_or((hostport, ""));
            (h.to_string(), Some(parse_port(p)?))
        } else {
            // Plain host name, or a bare IPv6 address without a port.
            (hostport.to_string(), None)
        };
        if host.is_empty() {
            return Err(Error::invalid(i18n::host_required()));
        }
        Ok(Destination { user, host, port })
    }

    /// A transient server entry for quick connections.
    pub fn to_server(&self) -> Server {
        Server {
            name: self.host.clone(),
            host: self.host.clone(),
            port: self.port.unwrap_or(DEFAULT_PORT),
            user: self.user.clone().unwrap_or_default(),
            ..Server::default()
        }
    }
}

fn parse_port(p: &str) -> Result<u16> {
    match p.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(Error::invalid(i18n::invalid_port(p))),
    }
}

/// Terminal size in character cells (and optionally pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtySize {
    pub cols: u32,
    pub rows: u32,
    #[serde(default)]
    pub width_px: u32,
    #[serde(default)]
    pub height_px: u32,
}

impl PtySize {
    pub fn new(cols: u32, rows: u32) -> Self {
        PtySize {
            cols: cols.clamp(1, 2000),
            rows: rows.clamp(1, 1000),
            width_px: 0,
            height_px: 0,
        }
    }
}

impl Default for PtySize {
    fn default() -> Self {
        PtySize::new(80, 24)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_destinations() {
        let d = Destination::parse("root@10.0.0.1:2222").unwrap();
        assert_eq!(d.user.as_deref(), Some("root"));
        assert_eq!(d.host, "10.0.0.1");
        assert_eq!(d.port, Some(2222));

        let d = Destination::parse("example.com").unwrap();
        assert_eq!(d.user, None);
        assert_eq!(d.port, None);

        let d = Destination::parse("ssh://deploy@[fe80::1]:22").unwrap();
        assert_eq!(d.user.as_deref(), Some("deploy"));
        assert_eq!(d.host, "fe80::1");
        assert_eq!(d.port, Some(22));

        let d = Destination::parse("admin@corp.local@bastion").unwrap();
        assert_eq!(d.user.as_deref(), Some("admin@corp.local"));
        assert_eq!(d.host, "bastion");

        let d = Destination::parse("::1").unwrap();
        assert_eq!(d.host, "::1");
        assert_eq!(d.port, None);

        assert!(Destination::parse("host:0").is_err());
        assert!(Destination::parse("host:abc").is_err());
        assert!(Destination::parse("").is_err());
        assert!(Destination::parse("user@").is_err());
    }

    #[test]
    fn normalize_server() {
        let mut s = Server {
            host: "  example.com ".into(),
            ..Server::default()
        };
        s.normalize().unwrap();
        assert_eq!(s.host, "example.com");
        assert_eq!(s.name, "example.com");

        let mut s = Server {
            host: "h".into(),
            auth: AuthKind::Key,
            ..Server::default()
        };
        assert!(s.normalize().is_err());
    }

    #[test]
    fn destination_formatting() {
        let s = Server {
            host: "fe80::1".into(),
            user: "me".into(),
            port: 2200,
            ..Server::default()
        };
        assert_eq!(s.destination(), "me@[fe80::1]:2200");
    }

    #[test]
    fn server_json_is_compact_and_tolerant() {
        let s: Server = serde_json::from_str(r#"{"id":"x","host":"h"}"#).unwrap();
        assert_eq!(s.port, 22);
        assert_eq!(s.auth, AuthKind::Auto);
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("identityFile"));
    }
}
