//! Importing hosts from the OpenSSH client configuration (`~/.ssh/config`).
//!
//! Follows OpenSSH semantics where it matters for import: case-insensitive keywords,
//! `key value` and `key=value` syntax, quoted arguments, `Include` with globs,
//! wildcard/negated `Host` patterns and "first obtained value wins".
//! `Match` blocks are skipped (except `Match all`).

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::model::{AuthKind, ForwardKind, ForwardSpec, Server};
use crate::util;

const MAX_INCLUDE_DEPTH: usize = 16;

/// Effective settings of one concrete `Host` alias.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostConfig {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_files: Vec<String>,
    pub identities_only: Option<bool>,
    pub proxy_jump: Option<String>,
    pub proxy_command: Option<String>,
    pub server_alive_interval: Option<u32>,
    pub connect_timeout: Option<u32>,
    pub preferred_authentications: Option<String>,
    pub local_forwards: Vec<Vec<String>>,
    pub remote_forwards: Vec<Vec<String>>,
    pub dynamic_forwards: Vec<Vec<String>>,
}

/// Servers produced by an import plus human-readable notes about skipped entries.
#[derive(Debug, Clone, Default)]
pub struct ImportResult {
    pub servers: Vec<Server>,
    pub skipped: Vec<String>,
}

#[derive(Debug)]
enum Condition {
    Always,
    Host(Vec<String>),
    Never,
}

#[derive(Debug)]
struct Block {
    condition: Condition,
    options: Vec<(String, Vec<String>)>,
}

/// Default location of the user's config.
pub fn default_path() -> Option<PathBuf> {
    util::ssh_dir().map(|d| d.join("config"))
}

/// Parses `path` (following `Include`s) and returns every concrete host alias.
pub fn load(path: &Path) -> Result<Vec<HostConfig>> {
    let text = std::fs::read_to_string(path)?;
    let base =
        util::ssh_dir().unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).to_path_buf());
    Ok(resolve_all(&parse(&text, &base)))
}

/// Parses config text; relative `Include` paths are resolved against `base`.
fn parse(text: &str, base: &Path) -> Vec<Block> {
    let mut blocks = vec![Block {
        condition: Condition::Always,
        options: Vec::new(),
    }];
    parse_into(text, base, 0, &mut blocks);
    blocks
}

fn parse_into(text: &str, base: &Path, depth: usize, blocks: &mut Vec<Block>) {
    for line in text.lines() {
        let Some((key, args)) = parse_line(line) else {
            continue;
        };
        match key.as_str() {
            "host" => blocks.push(Block {
                condition: Condition::Host(args),
                options: Vec::new(),
            }),
            "match" => {
                let all = args.len() == 1 && args[0].eq_ignore_ascii_case("all");
                blocks.push(Block {
                    condition: if all {
                        Condition::Always
                    } else {
                        Condition::Never
                    },
                    options: Vec::new(),
                });
            }
            "include" if depth < MAX_INCLUDE_DEPTH => {
                for pattern in &args {
                    for file in expand_include(pattern, base) {
                        if let Ok(included) = std::fs::read_to_string(&file) {
                            parse_into(&included, base, depth + 1, blocks);
                        }
                    }
                }
            }
            _ => {
                if let Some(block) = blocks.last_mut() {
                    block.options.push((key, args));
                }
            }
        }
    }
}

/// Splits a line into a lowercase keyword and its arguments.
fn parse_line(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let key = line[..end].to_ascii_lowercase();
    let mut rest = line[end..].trim_start();
    if let Some(r) = rest.strip_prefix('=') {
        rest = r.trim_start();
    }
    Some((key, split_args(rest)))
}

fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut has_token = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        args.push(current);
    }
    args
}

fn has_wildcard(s: &str) -> bool {
    s.contains('*') || s.contains('?')
}

fn expand_include(pattern: &str, base: &Path) -> Vec<PathBuf> {
    let mut path = util::expand_tilde(pattern);
    if path.is_relative() {
        path = base.join(path);
    }
    let file_name = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !has_wildcard(&file_name) {
        return if path.is_file() {
            vec![path]
        } else {
            Vec::new()
        };
    }
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| util::wildcard_match(&file_name, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    files
}

/// All concrete aliases (no wildcards or negation), in file order.
fn aliases(blocks: &[Block]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for block in blocks {
        if let Condition::Host(patterns) = &block.condition {
            for p in patterns {
                if !has_wildcard(p) && !p.starts_with('!') && !out.iter().any(|a| a == p) {
                    out.push(p.clone());
                }
            }
        }
    }
    out
}

fn resolve_all(blocks: &[Block]) -> Vec<HostConfig> {
    aliases(blocks)
        .into_iter()
        .map(|alias| resolve(blocks, &alias))
        .collect()
}

fn set_once<T>(slot: &mut Option<T>, value: Option<T>) {
    if slot.is_none() {
        *slot = value;
    }
}

fn yes_no(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "yes" | "true" => Some(true),
        "no" | "false" => Some(false),
        _ => None,
    }
}

/// Effective configuration for `alias` (first obtained value wins).
fn resolve(blocks: &[Block], alias: &str) -> HostConfig {
    let mut cfg = HostConfig {
        alias: alias.to_string(),
        ..HostConfig::default()
    };
    for block in blocks {
        let applies = match &block.condition {
            Condition::Always => true,
            Condition::Never => false,
            Condition::Host(patterns) => {
                util::pattern_list_match(patterns.iter().map(String::as_str), alias)
            }
        };
        if !applies {
            continue;
        }
        for (key, args) in &block.options {
            let first = args.first().cloned();
            match key.as_str() {
                "hostname" => set_once(&mut cfg.hostname, first),
                "user" => set_once(&mut cfg.user, first),
                "port" => set_once(&mut cfg.port, first.and_then(|p| p.parse().ok())),
                "identityfile" => cfg.identity_files.extend(first),
                "identitiesonly" => {
                    set_once(&mut cfg.identities_only, first.as_deref().and_then(yes_no))
                }
                "proxyjump" => set_once(&mut cfg.proxy_jump, first),
                "proxycommand" => set_once(&mut cfg.proxy_command, Some(args.join(" "))),
                "serveraliveinterval" => set_once(
                    &mut cfg.server_alive_interval,
                    first.and_then(|v| v.parse().ok()),
                ),
                "connecttimeout" => {
                    set_once(&mut cfg.connect_timeout, first.and_then(|v| v.parse().ok()))
                }
                "preferredauthentications" => set_once(&mut cfg.preferred_authentications, first),
                "localforward" => cfg.local_forwards.push(args.clone()),
                "remoteforward" => cfg.remote_forwards.push(args.clone()),
                "dynamicforward" => cfg.dynamic_forwards.push(args.clone()),
                _ => {}
            }
        }
    }
    cfg
}

/// Expands OpenSSH `%` tokens used in `HostName` and `IdentityFile`.
fn expand_tokens(value: &str, cfg: &HostConfig, host: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('h') => out.push_str(host),
            Some('n') => out.push_str(&cfg.alias),
            Some('p') => out.push_str(&cfg.port.unwrap_or(22).to_string()),
            Some('r') => out.push_str(cfg.user.as_deref().unwrap_or("")),
            Some('u') => out.push_str(&util::local_username()),
            Some('d') => out.push_str(
                &util::home_dir()
                    .map(|h| h.display().to_string())
                    .unwrap_or_default(),
            ),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Splits `[addr:]port`, `addr:port` or `[v6]:port`.
fn split_listen(spec: &str) -> Option<(Option<String>, u16)> {
    if let Some(rest) = spec.strip_prefix('[') {
        let (addr, port) = rest.split_once("]:")?;
        return Some((Some(addr.to_string()), port.parse().ok()?));
    }
    match spec.rsplit_once(':') {
        Some((addr, port)) => Some((Some(addr.to_string()), port.parse().ok()?)),
        None => Some((None, spec.parse().ok()?)),
    }
}

fn split_target(spec: &str) -> Option<(String, u16)> {
    let (host, port) = split_listen(spec)?;
    Some((host?, port))
}

fn bind_host(addr: Option<String>, default: &str) -> String {
    match addr.as_deref() {
        None | Some("") | Some("localhost") => default.to_string(),
        Some("*") => "0.0.0.0".to_string(),
        Some(a) => a.to_string(),
    }
}

fn forwards(cfg: &HostConfig) -> Vec<ForwardSpec> {
    let mut out = Vec::new();
    let pairs = cfg
        .local_forwards
        .iter()
        .map(|a| (ForwardKind::Local, a))
        .chain(cfg.remote_forwards.iter().map(|a| (ForwardKind::Remote, a)));
    for (kind, args) in pairs {
        let (Some(listen), Some(target)) = (args.first(), args.get(1)) else {
            continue;
        };
        let (Some((addr, bind_port)), Some((target_host, target_port))) =
            (split_listen(listen), split_target(target))
        else {
            continue;
        };
        let default_bind = if kind == ForwardKind::Remote {
            "localhost"
        } else {
            "127.0.0.1"
        };
        out.push(ForwardSpec {
            kind,
            bind_host: bind_host(addr, default_bind),
            bind_port,
            target_host,
            target_port,
        });
    }
    for args in &cfg.dynamic_forwards {
        if let Some((addr, bind_port)) = args.first().and_then(|a| split_listen(a)) {
            out.push(ForwardSpec {
                kind: ForwardKind::Dynamic,
                bind_host: bind_host(addr, "127.0.0.1"),
                bind_port,
                target_host: String::new(),
                target_port: 0,
            });
        }
    }
    out
}

/// Converts parsed hosts into server entries placed in `group`.
pub fn to_servers(configs: Vec<HostConfig>, group: &str) -> ImportResult {
    let mut result = ImportResult::default();
    for cfg in configs {
        if cfg
            .proxy_command
            .as_deref()
            .is_some_and(|c| !c.eq_ignore_ascii_case("none"))
        {
            result
                .skipped
                .push(format!("{}: ProxyCommand is not supported", cfg.alias));
            continue;
        }
        let host = expand_tokens(
            cfg.hostname.as_deref().unwrap_or(&cfg.alias),
            &cfg,
            &cfg.alias,
        );
        let identity_file = cfg
            .identity_files
            .iter()
            .map(|f| util::expand_tilde(&expand_tokens(f, &cfg, &host)))
            .find(|p| p.is_file())
            .map(|p| util::contract_tilde(&p));
        let prefers_password = cfg
            .preferred_authentications
            .as_deref()
            .is_some_and(|p| p.starts_with("password") || p.starts_with("keyboard-interactive"));
        let auth = if prefers_password {
            AuthKind::Password
        } else if cfg.identities_only == Some(true) && identity_file.is_some() {
            AuthKind::Key
        } else {
            AuthKind::Auto
        };
        let jump_host = cfg
            .proxy_jump
            .clone()
            .filter(|j| !j.eq_ignore_ascii_case("none"));
        result.servers.push(Server {
            name: cfg.alias.clone(),
            alias: Some(cfg.alias.clone()),
            host,
            port: cfg.port.unwrap_or(22),
            user: cfg.user.clone().unwrap_or_default(),
            group: group.to_string(),
            auth,
            identity_file,
            jump_host,
            keepalive_secs: cfg.server_alive_interval,
            connect_timeout_secs: cfg.connect_timeout,
            forwards: forwards(&cfg),
            ..Server::default()
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# Global defaults
ServerAliveInterval 15

Host web-01 web-02
    HostName 192.168.1.%h
    User deploy
    Port=2222

Host db
    HostName "db.internal"
    ProxyJump bastion
    LocalForward 5433 localhost:5432
    DynamicForward 1080

Host bastion
    HostName bastion.example.com
    User jump
    IdentitiesOnly yes

Host legacy
    HostName 10.0.0.9
    PreferredAuthentications password
    ServerAliveInterval 60

Host aws-*
    ProxyCommand aws ssm start-session --target %h

Host aws-box
    User ec2-user

Match exec "true"
    User nobody

Host *
    User fallback
    Port 22
"#;

    #[test]
    fn parses_hosts_with_openssh_semantics() {
        let blocks = parse(SAMPLE, Path::new("/nonexistent"));
        let hosts = resolve_all(&blocks);
        let names: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(
            names,
            ["web-01", "web-02", "db", "bastion", "legacy", "aws-box"]
        );

        let web = &hosts[0];
        assert_eq!(web.hostname.as_deref(), Some("192.168.1.%h"));
        assert_eq!(web.user.as_deref(), Some("deploy"));
        assert_eq!(web.port, Some(2222));
        assert_eq!(web.server_alive_interval, Some(15));

        let db = &hosts[2];
        assert_eq!(db.hostname.as_deref(), Some("db.internal"));
        assert_eq!(db.user.as_deref(), Some("fallback"));
        assert_eq!(db.proxy_jump.as_deref(), Some("bastion"));

        let legacy = &hosts[4];
        // "First obtained value wins": the global 15 comes before the host's 60.
        assert_eq!(legacy.server_alive_interval, Some(15));
    }

    #[test]
    fn converts_to_servers() {
        let hosts = resolve_all(&parse(SAMPLE, Path::new("/nonexistent")));
        let result = to_servers(hosts, "SSH config");
        assert_eq!(result.skipped.len(), 1, "aws-box uses ProxyCommand");
        let by_alias = |a: &str| {
            result
                .servers
                .iter()
                .find(|s| s.alias.as_deref() == Some(a))
                .unwrap()
        };

        let web = by_alias("web-01");
        assert_eq!(web.host, "192.168.1.web-01");
        assert_eq!(web.port, 2222);
        assert_eq!(web.group, "SSH config");

        let db = by_alias("db");
        assert_eq!(db.jump_host.as_deref(), Some("bastion"));
        assert_eq!(db.forwards.len(), 2);
        assert_eq!(db.forwards[0].kind, ForwardKind::Local);
        assert_eq!(db.forwards[0].bind_port, 5433);
        assert_eq!(db.forwards[0].target_host, "localhost");
        assert_eq!(db.forwards[0].target_port, 5432);
        assert_eq!(db.forwards[1].kind, ForwardKind::Dynamic);
        assert_eq!(db.forwards[1].bind_port, 1080);

        assert_eq!(by_alias("legacy").auth, AuthKind::Password);
        // IdentitiesOnly without an existing key file stays on Auto.
        assert_eq!(by_alias("bastion").auth, AuthKind::Auto);
    }

    #[test]
    fn follows_includes_with_globs() {
        let dir = std::env::temp_dir().join(format!("nexssh-sshcfg-{}", util::random_id()));
        std::fs::create_dir_all(dir.join("conf.d")).unwrap();
        std::fs::write(
            dir.join("conf.d/a.conf"),
            "Host inc-a\n  HostName a.example\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("conf.d/b.conf"),
            "Host inc-b\n  HostName b.example\n",
        )
        .unwrap();
        let main = "Include conf.d/*.conf\nHost main\n  HostName m.example\n";
        let hosts = resolve_all(&parse(main, &dir));
        let names: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(names, ["inc-a", "inc-b", "main"]);
    }

    #[test]
    fn line_syntax() {
        assert_eq!(
            parse_line("  IdentityFile \"~/.ssh/my key\"  "),
            Some(("identityfile".into(), vec!["~/.ssh/my key".into()]))
        );
        assert_eq!(
            parse_line("Port=22"),
            Some(("port".into(), vec!["22".into()]))
        );
        assert_eq!(
            parse_line("Port = 22"),
            Some(("port".into(), vec!["22".into()]))
        );
        assert_eq!(parse_line("# comment"), None);
        assert_eq!(split_listen("[::1]:8080"), Some((Some("::1".into()), 8080)));
        assert_eq!(split_listen("*:8080"), Some((Some("*".into()), 8080)));
        assert_eq!(split_listen("8080"), Some((None, 8080)));
    }
}
