use std::fmt;

use crate::i18n;

/// Result type used across the core crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors produced by the core. Messages are written to be shown to users as-is, in the
/// current [`i18n`] language.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("{}", i18n::invalid_data(.0))]
    Json(#[from] serde_json::Error),

    #[error("{}", ssh_error_message(.0))]
    Ssh(#[from] russh::Error),

    #[error("{}", i18n::key_error(.0))]
    Key(#[from] russh::keys::Error),

    #[error("{}", i18n::keychain_error(.0))]
    Secret(String),

    #[error("{0}")]
    Invalid(String),

    /// A complete message, e.g. from [`i18n::session_not_found`].
    #[error("{0}")]
    NotFound(String),

    /// The server refused access to a file (a complete message): writing it may take sudo.
    #[error("{0}")]
    Denied(String),

    #[error("{}", i18n::timed_out_after(*.0))]
    Timeout(u32),

    #[error("{}", i18n::auth_failed(.0))]
    AuthFailed(String),

    #[error("{}", i18n::host_key_failed(.0))]
    HostKey(String),

    #[error("{}", i18n::cancelled())]
    Cancelled,

    /// A complete message describing why the connection ended or could not be made.
    #[error("{0}")]
    Disconnected(String),
}

impl Error {
    pub fn invalid(msg: impl fmt::Display) -> Self {
        Error::Invalid(msg.to_string())
    }
}

/// russh error messages are terse and sometimes cryptic; translate the common ones.
fn ssh_error_message(err: &russh::Error) -> String {
    use russh::Error as E;
    match err {
        E::ConnectionTimeout => i18n::ssh_timeout(),
        E::KeepaliveTimeout => i18n::ssh_keepalive_timeout(),
        E::InactivityTimeout => i18n::ssh_inactivity(),
        E::UnknownKey => i18n::ssh_unknown_key(),
        E::Disconnect => i18n::ssh_disconnected(),
        E::HUP => i18n::ssh_hangup(),
        E::NoCommonAlgo { .. } => i18n::ssh_no_common_algorithm(err),
        E::IO(io) => io.to_string(),
        other => other.to_string(),
    }
}
