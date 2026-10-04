//! Small helpers shared by the core modules.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A random, URL-safe identifier (16 hex chars, 64 bits of entropy).
pub fn random_id() -> String {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        // Extremely unlikely; fall back to something unique enough for local IDs.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        bytes = (nanos as u64).to_le_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// `~/.ssh`
pub fn ssh_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".ssh"))
}

/// Expands a leading `~` to the home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    } else if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

/// Replaces the home directory prefix with `~` for display purposes.
pub fn contract_tilde(path: &Path) -> String {
    if let Some(home) = home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        let rest = rest.to_string_lossy();
        return if rest.is_empty() {
            "~".into()
        } else {
            format!("~/{}", rest.replace('\\', "/"))
        };
    }
    path.to_string_lossy().into_owned()
}

/// The name of the local user: what OpenSSH logs in as when a server entry has no username,
/// which NexSSH suggests when it asks for one.
pub fn local_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".into())
}

/// OpenSSH-style wildcard matching: `*` matches any sequence, `?` any single character.
/// Matching is case-insensitive, as host names are.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Matches `text` against a comma- or whitespace-separated OpenSSH pattern list,
/// honouring `!negated` entries. Returns true when at least one positive pattern
/// matches and no negated pattern does.
pub fn pattern_list_match<'a>(patterns: impl IntoIterator<Item = &'a str>, text: &str) -> bool {
    let mut matched = false;
    for pat in patterns {
        if let Some(neg) = pat.strip_prefix('!') {
            if wildcard_match(neg, text) {
                return false;
            }
        } else if wildcard_match(pat, text) {
            matched = true;
        }
    }
    matched
}

/// Writes a file atomically: write to a temporary sibling, fsync, then rename over the target.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", random_id()));
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Formats `host:port`, bracketing IPv6 literals.
pub fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("*.example.com", "web.example.com"));
        assert!(!wildcard_match("*.example.com", "example.com"));
        assert!(wildcard_match("web-??", "web-01"));
        assert!(!wildcard_match("web-??", "web-1"));
        assert!(wildcard_match("WEB*", "web-01"));
        assert!(wildcard_match("a*b*c", "aXXbYYc"));
        assert!(!wildcard_match("a*b*c", "aXXbYY"));
    }

    #[test]
    fn pattern_lists() {
        assert!(pattern_list_match(["*.corp", "!secret.corp"], "a.corp"));
        assert!(!pattern_list_match(
            ["*.corp", "!secret.corp"],
            "secret.corp"
        ));
        assert!(!pattern_list_match(["!foo"], "bar"));
    }

    #[test]
    fn host_port_formatting() {
        assert_eq!(host_port("example.com", 22), "example.com:22");
        assert_eq!(host_port("::1", 2222), "[::1]:2222");
    }

    #[test]
    fn random_ids_are_unique() {
        let a = random_id();
        let b = random_id();
        assert_eq!(a.len(), 16);
        assert_ne!(a, b);
    }
}
