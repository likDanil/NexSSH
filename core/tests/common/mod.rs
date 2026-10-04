//! Shared helpers for the integration tests: OpenSSH servers started by
//! `scripts/test-sshd.sh`, an isolated `Core` and a session driver.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use nexssh_core::known_hosts::KnownHosts;
use nexssh_core::secrets::Secrets;
use nexssh_core::{
    AuthKind, Core, EventSink, LocalCommand, Prompt, PromptReply, PtySize, Server, SessionEvent,
    SessionId, SessionStatus,
};
use tokio::sync::mpsc;

pub struct Env {
    pub host: String,
    pub port: u16,
    pub kbd_port: u16,
    pub user: String,
    pub password: String,
    pub key: String,
    pub enc_key: String,
    pub rsa_key: String,
    pub passphrase: String,
}

pub fn env() -> Option<Env> {
    let var = |k: &str| std::env::var(k).ok();
    Some(Env {
        host: var("NEXSSH_TEST_HOST")?,
        port: var("NEXSSH_TEST_PORT")?.parse().ok()?,
        kbd_port: var("NEXSSH_TEST_KBD_PORT")?.parse().ok()?,
        user: var("NEXSSH_TEST_USER")?,
        password: var("NEXSSH_TEST_PASSWORD")?,
        key: var("NEXSSH_TEST_KEY")?,
        enc_key: var("NEXSSH_TEST_ENC_KEY")?,
        rsa_key: var("NEXSSH_TEST_RSA_KEY")?,
        passphrase: var("NEXSSH_TEST_PASSPHRASE")?,
    })
}

pub enum Ev {
    Event(SessionEvent),
    Output(Vec<u8>),
}

pub struct Sink(pub mpsc::UnboundedSender<Ev>);

impl EventSink for Sink {
    fn event(&self, event: SessionEvent) {
        let _ = self.0.send(Ev::Event(event));
    }
    fn output(&self, data: Vec<u8>) {
        let _ = self.0.send(Ev::Output(data));
    }
}

pub fn temp_dir() -> PathBuf {
    // Tests running at the same time can see the same clock (Windows' is coarse).
    static TAKEN: AtomicU64 = AtomicU64::new(0);
    let n: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let dir = std::env::temp_dir().join(format!(
        "nexssh-it-{n}-{}-{}",
        std::process::id(),
        TAKEN.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A core isolated from the user's ~/.ssh/known_hosts and OS keychain.
pub fn core() -> Core {
    let dir = temp_dir();
    let kh = KnownHosts::new(dir.join("known_hosts"), None);
    Core::with_options(&dir, Secrets::in_memory(), Some(kh)).unwrap()
}

pub struct Session {
    pub core: Arc<Core>,
    pub id: SessionId,
    pub rx: mpsc::UnboundedReceiver<Ev>,
    pub output: String,
    pub log: Vec<String>,
}

impl Session {
    pub fn open(core: Arc<Core>, server: Server) -> Session {
        let (tx, rx) = mpsc::unbounded_channel();
        let id = core
            .sessions
            .open(server, PtySize::new(100, 30), Arc::new(Sink(tx)));
        Session::new(core, id, rx)
    }

    /// A local terminal running `command`.
    pub fn open_local(core: Arc<Core>, command: LocalCommand) -> Session {
        let (tx, rx) = mpsc::unbounded_channel();
        let id = core
            .sessions
            .open_local(command, PtySize::new(100, 30), Arc::new(Sink(tx)));
        Session::new(core, id, rx)
    }

    fn new(core: Arc<Core>, id: SessionId, rx: mpsc::UnboundedReceiver<Ev>) -> Session {
        Session {
            core,
            id,
            rx,
            output: String::new(),
            log: Vec::new(),
        }
    }

    pub async fn next_event(&mut self) -> SessionEvent {
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(20), self.rx.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!("timed out; log: {:?}; output: {}", self.log, self.output)
                })
                .expect("session event stream ended");
            match ev {
                Ev::Output(data) => self.output.push_str(&String::from_utf8_lossy(&data)),
                Ev::Event(SessionEvent::Log { message, .. }) => self.log.push(message),
                Ev::Event(SessionEvent::Forwards { .. }) => {}
                Ev::Event(SessionEvent::PromptClosed { .. }) => {}
                Ev::Event(SessionEvent::Status {
                    status: SessionStatus::Connecting,
                    ..
                }) => {}
                Ev::Event(e) => return e,
            }
        }
    }

    pub async fn expect_prompt(&mut self) -> (u64, Prompt) {
        match self.next_event().await {
            SessionEvent::Prompt { id, prompt } => (id, prompt),
            other => panic!("expected a prompt, got {other:?}; log: {:?}", self.log),
        }
    }

    pub async fn expect_status(&mut self, want: SessionStatus) -> Option<String> {
        self.expect_status_full(want).await.0
    }

    /// The status message and whether the status reports a failure.
    pub async fn expect_status_full(&mut self, want: SessionStatus) -> (Option<String>, bool) {
        loop {
            match self.next_event().await {
                SessionEvent::Status {
                    status,
                    message,
                    failed,
                } if status == want => return (message, failed),
                SessionEvent::Status {
                    status: SessionStatus::Connecting,
                    ..
                } => {}
                other => panic!("expected {want:?}, got {other:?}; log: {:?}", self.log),
            }
        }
    }

    pub fn answer(&self, id: u64, reply: PromptReply) {
        self.core.sessions.answer(id, reply).unwrap();
    }

    pub async fn accept_host_key(&mut self) {
        let (id, prompt) = self.expect_prompt().await;
        assert!(matches!(prompt, Prompt::HostKey { .. }), "got {prompt:?}");
        self.answer(
            id,
            PromptReply::HostKey {
                accept: true,
                remember: true,
            },
        );
    }

    pub fn write(&self, text: &str) {
        self.core
            .sessions
            .write(self.id, text.as_bytes().to_vec())
            .unwrap();
    }

    pub async fn wait_output(&mut self, needle: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while !self.output.contains(needle) {
            let ev = tokio::time::timeout_at(deadline, self.rx.recv())
                .await
                .unwrap_or_else(|_| panic!("'{needle}' not seen; output: {}", self.output))
                .unwrap();
            match ev {
                Ev::Output(data) => self.output.push_str(&String::from_utf8_lossy(&data)),
                Ev::Event(SessionEvent::Log { message, .. }) => self.log.push(message),
                Ev::Event(_) => {}
            }
        }
    }

    pub async fn close(mut self) {
        self.core.sessions.close(self.id);
        self.expect_status(SessionStatus::Closed).await;
    }
}

pub fn server(env: &Env, auth: AuthKind) -> Server {
    Server {
        name: "test".into(),
        host: env.host.clone(),
        port: env.port,
        user: env.user.clone(),
        auth,
        ..Server::default()
    }
}
