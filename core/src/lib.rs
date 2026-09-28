//! NexSSH core: everything that is not UI.
//!
//! * [`store`] — saved servers (`servers.json`, no secrets inside)
//! * [`secrets`] — passwords/passphrases in the OS keychain
//! * [`known_hosts`] — host key verification (OpenSSH compatible)
//! * [`ssh_config`] — import from `~/.ssh/config`
//! * [`keys`] — local private key discovery and loading
//! * [`session`] — interactive SSH sessions (jump hosts, auth, PTY, reconnect)
//! * [`forward`] — local/remote/dynamic port forwarding
//!
//! The crate has no dependency on Tauri or any GUI toolkit: a front-end creates a
//! [`Core`], opens sessions with an [`EventSink`] and answers [`Prompt`]s.

pub mod error;
pub mod forward;
pub mod keys;
pub mod known_hosts;
pub mod model;
pub mod secrets;
pub mod session;
pub mod ssh_config;
pub mod store;
mod util;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use error::{Error, Result};
pub use model::{AuthKind, Destination, ForwardKind, ForwardSpec, PtySize, Server};
pub use session::{
    EventSink, KbdPrompt, LogLevel, Prompt, PromptReply, SessionEvent, SessionId, SessionManager,
    SessionStatus,
};
pub use store::{ImportSummary, ServerStore, StoreData};

pub use util::{contract_tilde, expand_tilde};

/// Default data directory: `%APPDATA%\NexSSH` on Windows,
/// `~/Library/Application Support/NexSSH` on macOS, `~/.config/nexssh` elsewhere.
/// `NEXSSH_DATA_DIR` overrides it (portable installs, tests).
pub fn default_data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("NEXSSH_DATA_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let name = if cfg!(any(windows, target_os = "macos")) {
        "NexSSH"
    } else {
        "nexssh"
    };
    dirs::config_dir().map(|d| d.join(name))
}

/// All core services, rooted at one data directory.
pub struct Core {
    pub data_dir: PathBuf,
    pub store: Arc<ServerStore>,
    pub secrets: Arc<secrets::Secrets>,
    pub known_hosts: Arc<known_hosts::KnownHosts>,
    pub sessions: SessionManager,
}

impl Core {
    /// Opens (creating if needed) the data directory with `servers.json` and `known_hosts`.
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Core> {
        Core::with_secrets(data_dir, secrets::Secrets::system())
    }

    /// Like [`Core::open`] with a custom secret store (e.g. in-memory for tests).
    pub fn with_secrets(data_dir: impl AsRef<Path>, secrets: secrets::Secrets) -> Result<Core> {
        Core::with_options(data_dir, secrets, None)
    }

    /// Full control over the secret store and host key files (`None` = defaults:
    /// `<data_dir>/known_hosts` plus the user's `~/.ssh/known_hosts`).
    pub fn with_options(
        data_dir: impl AsRef<Path>,
        secrets: secrets::Secrets,
        known_hosts: Option<known_hosts::KnownHosts>,
    ) -> Result<Core> {
        let data_dir = data_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&data_dir)?;
        let store = Arc::new(ServerStore::open(data_dir.join("servers.json"))?);
        let secrets = Arc::new(secrets);
        let known_hosts = Arc::new(
            known_hosts.unwrap_or_else(|| known_hosts::KnownHosts::with_defaults(&data_dir)),
        );
        let sessions = SessionManager::new(
            Arc::clone(&store),
            Arc::clone(&secrets),
            Arc::clone(&known_hosts),
        );
        Ok(Core {
            data_dir,
            store,
            secrets,
            known_hosts,
            sessions,
        })
    }

    /// Imports hosts from an ssh config file (default `~/.ssh/config`) into `group`.
    pub fn import_ssh_config(&self, path: Option<&Path>, group: &str) -> Result<ImportReport> {
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => ssh_config::default_path()
                .ok_or_else(|| Error::NotFound("home directory".into()))?,
        };
        if !path.is_file() {
            return Err(Error::NotFound(contract_tilde(&path)));
        }
        let hosts = ssh_config::load(&path)?;
        let found = hosts.len();
        let result = ssh_config::to_servers(hosts, group);
        let summary = self.store.merge_imported(result.servers)?;
        Ok(ImportReport {
            path: contract_tilde(&path),
            found,
            summary,
            skipped: result.skipped,
        })
    }

    /// Deletes a server together with its saved password.
    pub fn delete_server(&self, id: &str) -> Result<()> {
        self.store.delete_server(id)?;
        let _ = self.secrets.delete(&secrets::Secrets::password_account(id));
        Ok(())
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub path: String,
    pub found: usize,
    pub summary: ImportSummary,
    pub skipped: Vec<String>,
}
