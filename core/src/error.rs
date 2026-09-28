use std::fmt;

/// Result type used across the core crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors produced by the core. Messages are written to be shown to users as-is.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("invalid data: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{}", ssh_error_message(.0))]
    Ssh(#[from] russh::Error),

    #[error("key error: {0}")]
    Key(#[from] russh::keys::Error),

    #[error("system keychain: {0}")]
    Secret(String),

    #[error("{0}")]
    Invalid(String),

    #[error("{0} not found")]
    NotFound(String),

    #[error("connection timed out after {0}s")]
    Timeout(u32),

    #[error("authentication failed ({0})")]
    AuthFailed(String),

    #[error("host key verification failed: {0}")]
    HostKey(String),

    #[error("cancelled")]
    Cancelled,

    #[error("connection closed: {0}")]
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
        E::ConnectionTimeout => "connection timed out".into(),
        E::KeepaliveTimeout => "server stopped responding (keepalive timeout)".into(),
        E::InactivityTimeout => "connection closed after inactivity".into(),
        E::UnknownKey => "server host key was not accepted".into(),
        E::Disconnect => "disconnected".into(),
        E::HUP => "connection closed by remote host".into(),
        E::NoCommonAlgo { .. } => format!("no algorithm in common with the server ({err})"),
        E::IO(io) => io.to_string(),
        other => other.to_string(),
    }
}
