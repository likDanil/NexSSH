//! Integration tests against real OpenSSH servers started by `scripts/test-sshd.sh`.
//! They are skipped (pass trivially) unless `NEXSSH_TEST_HOST` is set.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nexssh_core::known_hosts::KnownHosts;
use nexssh_core::secrets::Secrets;
use nexssh_core::{
    AuthKind, Core, EventSink, ForwardKind, ForwardSpec, Prompt, PromptReply, PtySize, Server,
    SessionEvent, SessionId, SessionStatus,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

struct Env {
    host: String,
    port: u16,
    kbd_port: u16,
    user: String,
    password: String,
    key: String,
    enc_key: String,
    rsa_key: String,
    passphrase: String,
}

fn env() -> Option<Env> {
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

enum Ev {
    Event(SessionEvent),
    Output(Vec<u8>),
}

struct Sink(mpsc::UnboundedSender<Ev>);

impl EventSink for Sink {
    fn event(&self, event: SessionEvent) {
        let _ = self.0.send(Ev::Event(event));
    }
    fn output(&self, data: Vec<u8>) {
        let _ = self.0.send(Ev::Output(data));
    }
}

fn temp_dir() -> PathBuf {
    let n: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let dir = std::env::temp_dir().join(format!("nexssh-it-{n}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A core isolated from the user's ~/.ssh/known_hosts and OS keychain.
fn core() -> Core {
    let dir = temp_dir();
    let kh = KnownHosts::new(dir.join("known_hosts"), None);
    Core::with_options(&dir, Secrets::in_memory(), Some(kh)).unwrap()
}

struct Session {
    core: Arc<Core>,
    id: SessionId,
    rx: mpsc::UnboundedReceiver<Ev>,
    output: String,
    log: Vec<String>,
}

impl Session {
    fn open(core: Arc<Core>, server: Server) -> Session {
        let (tx, rx) = mpsc::unbounded_channel();
        let id = core
            .sessions
            .open(server, PtySize::new(100, 30), Arc::new(Sink(tx)));
        Session {
            core,
            id,
            rx,
            output: String::new(),
            log: Vec::new(),
        }
    }

    async fn next_event(&mut self) -> SessionEvent {
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

    async fn expect_prompt(&mut self) -> (u64, Prompt) {
        match self.next_event().await {
            SessionEvent::Prompt { id, prompt } => (id, prompt),
            other => panic!("expected a prompt, got {other:?}; log: {:?}", self.log),
        }
    }

    async fn expect_status(&mut self, want: SessionStatus) -> Option<String> {
        loop {
            match self.next_event().await {
                SessionEvent::Status { status, message } if status == want => return message,
                SessionEvent::Status {
                    status: SessionStatus::Connecting,
                    ..
                } => {}
                other => panic!("expected {want:?}, got {other:?}; log: {:?}", self.log),
            }
        }
    }

    fn answer(&self, id: u64, reply: PromptReply) {
        self.core.sessions.answer(id, reply).unwrap();
    }

    async fn accept_host_key(&mut self) {
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

    fn write(&self, text: &str) {
        self.core
            .sessions
            .write(self.id, text.as_bytes().to_vec())
            .unwrap();
    }

    async fn wait_output(&mut self, needle: &str) {
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

    async fn close(mut self) {
        self.core.sessions.close(self.id);
        self.expect_status(SessionStatus::Closed).await;
    }
}

fn server(env: &Env, auth: AuthKind) -> Server {
    Server {
        name: "test".into(),
        host: env.host.clone(),
        port: env.port,
        user: env.user.clone(),
        auth,
        ..Server::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn password_shell_resize_exit_and_reconnect() {
    let Some(env) = env() else { return };
    let core = Arc::new(core());
    // A saved server, so the password can be remembered in the (in-memory) keychain.
    let saved = core
        .store
        .save_server(server(&env, AuthKind::Password))
        .unwrap();
    let mut s = Session::open(Arc::clone(&core), saved.clone());
    s.accept_host_key().await;

    // Wrong password first: the prompt comes back with an error.
    let (id, prompt) = s.expect_prompt().await;
    assert!(matches!(
        prompt,
        Prompt::Password {
            can_remember: true,
            ..
        }
    ));
    s.answer(
        id,
        PromptReply::Secret {
            value: "wrong".into(),
            remember: false,
        },
    );
    let (id, prompt) = s.expect_prompt().await;
    match prompt {
        Prompt::Password { error, .. } => assert!(error.is_some()),
        other => panic!("{other:?}"),
    }
    s.answer(
        id,
        PromptReply::Secret {
            value: env.password.clone(),
            remember: true,
        },
    );
    s.expect_status(SessionStatus::Connected).await;
    assert!(core.secrets.has(&Secrets::password_account(&saved.id)));

    s.write("echo nexssh-$((40+2))\n");
    s.wait_output("nexssh-42").await;

    core.sessions.resize(s.id, PtySize::new(123, 45)).unwrap();
    s.write("stty size\n");
    s.wait_output("45 123").await;

    // Exiting the shell leaves the session reconnectable.
    s.write("exit 3\n");
    let reason = s.expect_status(SessionStatus::Disconnected).await.unwrap();
    assert!(reason.contains("exit code 3"), "{reason}");

    // Reconnect: host key is remembered and the password comes from the keychain,
    // so there are no prompts at all.
    s.output.clear();
    core.sessions.reconnect(s.id).unwrap();
    s.expect_status(SessionStatus::Connected).await;
    s.write("echo again-$((1+1))\n");
    s.wait_output("again-2").await;
    s.close().await;
    assert_eq!(core.sessions.session_count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn public_key_ed25519_and_rsa() {
    let Some(env) = env() else { return };
    let core = Arc::new(core());
    for key in [&env.key, &env.rsa_key] {
        let mut srv = server(&env, AuthKind::Key);
        srv.identity_file = Some(key.clone());
        let mut s = Session::open(Arc::clone(&core), srv);
        if core
            .known_hosts
            .known_algorithms(&env.host, env.port)
            .is_empty()
        {
            s.accept_host_key().await;
        }
        s.expect_status(SessionStatus::Connected).await;
        assert!(
            s.log.iter().any(|l| l.contains("Authenticated")),
            "{:?}",
            s.log
        );
        s.write("echo key-ok\n");
        s.wait_output("key-ok").await;
        s.close().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn encrypted_key_asks_passphrase_only_when_accepted() {
    let Some(env) = env() else { return };
    let core = Arc::new(core());
    let mut srv = server(&env, AuthKind::Key);
    srv.identity_file = Some(env.enc_key.clone());
    let mut s = Session::open(Arc::clone(&core), srv);
    s.accept_host_key().await;
    let (id, prompt) = s.expect_prompt().await;
    assert!(matches!(prompt, Prompt::Passphrase { .. }), "{prompt:?}");
    s.answer(
        id,
        PromptReply::Secret {
            value: "not it".into(),
            remember: false,
        },
    );
    let (id, prompt) = s.expect_prompt().await;
    match prompt {
        Prompt::Passphrase { error, .. } => assert!(error.is_some()),
        other => panic!("{other:?}"),
    }
    s.answer(
        id,
        PromptReply::Secret {
            value: env.passphrase.clone(),
            remember: false,
        },
    );
    s.expect_status(SessionStatus::Connected).await;
    s.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_passphrase_falls_back_to_password() {
    let Some(env) = env() else { return };
    let core = Arc::new(core());
    let mut srv = server(&env, AuthKind::Auto);
    srv.identity_file = Some(env.enc_key.clone());
    let mut s = Session::open(Arc::clone(&core), srv);
    s.accept_host_key().await;
    let (id, prompt) = s.expect_prompt().await;
    assert!(matches!(prompt, Prompt::Passphrase { .. }), "{prompt:?}");
    s.answer(id, PromptReply::Cancel);
    // The server rejected the attempt; authentication continues with the password.
    let (id, prompt) = s.expect_prompt().await;
    assert!(matches!(prompt, Prompt::Password { .. }), "{prompt:?}");
    s.answer(
        id,
        PromptReply::Secret {
            value: env.password.clone(),
            remember: false,
        },
    );
    s.expect_status(SessionStatus::Connected).await;
    s.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn keyboard_interactive_uses_password_dialog() {
    let Some(env) = env() else { return };
    let core = Arc::new(core());
    let mut srv = server(&env, AuthKind::Auto);
    srv.port = env.kbd_port;
    let mut s = Session::open(Arc::clone(&core), srv);
    s.accept_host_key().await;
    let (id, prompt) = s.expect_prompt().await;
    assert!(matches!(prompt, Prompt::Password { .. }), "{prompt:?}");
    s.answer(
        id,
        PromptReply::Secret {
            value: env.password.clone(),
            remember: false,
        },
    );
    s.expect_status(SessionStatus::Connected).await;
    assert!(
        s.log.iter().any(|l| l.contains("keyboard-interactive")),
        "{:?}",
        s.log
    );
    s.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_host_key_fails_cleanly() {
    let Some(env) = env() else { return };
    let core = Arc::new(core());
    let mut s = Session::open(Arc::clone(&core), server(&env, AuthKind::Password));
    let (id, _) = s.expect_prompt().await;
    s.answer(
        id,
        PromptReply::HostKey {
            accept: false,
            remember: false,
        },
    );
    let reason = s.expect_status(SessionStatus::Disconnected).await.unwrap();
    assert!(reason.contains("host key"), "{reason}");
    s.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jump_host_chain() {
    let Some(env) = env() else { return };
    let core = Arc::new(core());
    let mut jump = server(&env, AuthKind::Key);
    jump.name = "bastion".into();
    jump.identity_file = Some(env.key.clone());
    core.store.save_server(jump).unwrap();

    let mut target = server(&env, AuthKind::Key);
    target.identity_file = Some(env.key.clone());
    target.jump_host = Some("bastion".into());
    let mut s = Session::open(Arc::clone(&core), target);
    // Same host twice: the key is asked for once and then remembered.
    s.accept_host_key().await;
    s.expect_status(SessionStatus::Connected).await;
    assert!(s.log.iter().any(|l| l.contains("jump host")), "{:?}", s.log);
    s.write("echo via-jump\n");
    s.wait_output("via-jump").await;
    s.close().await;
}

async fn read_banner(port: u16) -> String {
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test(flavor = "multi_thread")]
async fn local_dynamic_and_remote_forwarding() {
    let Some(env) = env() else { return };
    let core = Arc::new(core());
    let mut srv = server(&env, AuthKind::Key);
    srv.identity_file = Some(env.key.clone());
    let local_port = free_port();
    // Saved forwards start automatically after connecting.
    srv.forwards = vec![ForwardSpec {
        kind: ForwardKind::Local,
        bind_host: "127.0.0.1".into(),
        bind_port: local_port,
        target_host: "127.0.0.1".into(),
        target_port: env.port,
    }];
    let mut s = Session::open(Arc::clone(&core), srv);
    s.accept_host_key().await;
    s.expect_status(SessionStatus::Connected).await;

    // -L: the local port reaches the SSH server itself through the tunnel.
    assert!(read_banner(local_port).await.starts_with("SSH-2.0-"));

    // -D: SOCKS5 CONNECT to the SSH server.
    let socks_port = free_port();
    core.sessions
        .add_forward(
            s.id,
            ForwardSpec {
                kind: ForwardKind::Dynamic,
                bind_host: "127.0.0.1".into(),
                bind_port: socks_port,
                target_host: String::new(),
                target_port: 0,
            },
        )
        .await
        .unwrap();
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", socks_port))
        .await
        .unwrap();
    sock.write_all(&[5, 1, 0]).await.unwrap();
    let mut reply = [0u8; 2];
    sock.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply, [5, 0]);
    let mut req = vec![5, 1, 0, 1, 127, 0, 0, 1];
    req.extend_from_slice(&env.port.to_be_bytes());
    sock.write_all(&req).await.unwrap();
    let mut head = [0u8; 10];
    sock.read_exact(&mut head).await.unwrap();
    assert_eq!(head[1], 0, "SOCKS reply code");
    let mut banner = [0u8; 8];
    sock.read_exact(&mut banner).await.unwrap();
    assert_eq!(&banner, b"SSH-2.0-");

    // -R: the server listens and connections come back to a local service.
    let service = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let service_port = service.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((mut c, _)) = service.accept().await {
            let _ = c.write_all(b"hello-from-local").await;
        }
    });
    let remote_port = free_port();
    core.sessions
        .add_forward(
            s.id,
            ForwardSpec {
                kind: ForwardKind::Remote,
                bind_host: "127.0.0.1".into(),
                bind_port: remote_port,
                target_host: "127.0.0.1".into(),
                target_port: service_port,
            },
        )
        .await
        .unwrap();
    let mut remote = tokio::net::TcpStream::connect(("127.0.0.1", remote_port))
        .await
        .unwrap();
    let mut got = vec![0u8; 16];
    tokio::time::timeout(Duration::from_secs(5), remote.read_exact(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&got, b"hello-from-local");

    s.close().await;
    // Listeners are gone once the session is closed.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", local_port))
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_keeps_session_reconnectable() {
    let Some(env) = env() else { return };
    let core = Arc::new(core());
    let mut srv = server(&env, AuthKind::Key);
    srv.identity_file = Some(env.key.clone());
    let mut s = Session::open(Arc::clone(&core), srv);
    s.accept_host_key().await;
    s.expect_status(SessionStatus::Connected).await;

    core.sessions.disconnect(s.id).unwrap();
    let reason = s.expect_status(SessionStatus::Disconnected).await.unwrap();
    assert_eq!(reason, "Disconnected");
    // Input while disconnected is ignored, the session is still there.
    s.write("echo ignored\n");
    core.sessions.reconnect(s.id).unwrap();
    s.expect_status(SessionStatus::Connected).await;
    s.write("echo back-again\n");
    s.wait_output("back-again").await;
    s.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn final_output_is_delivered_before_disconnect() {
    let Some(env) = env() else { return };
    let core = Arc::new(core());
    let mut srv = server(&env, AuthKind::Key);
    srv.identity_file = Some(env.key.clone());
    let mut s = Session::open(Arc::clone(&core), srv);
    s.accept_host_key().await;
    s.expect_status(SessionStatus::Connected).await;
    s.write("echo ready\n");
    s.wait_output("ready").await;
    s.output.clear();
    s.write("printf 'bye-bye\\n'; exit 0\n");
    s.expect_status(SessionStatus::Disconnected).await;
    eprintln!("OUTPUT AFTER EXIT: {:?}", s.output);
    assert!(s.output.contains("bye-bye"), "{:?}", s.output);
    s.close().await;
}
