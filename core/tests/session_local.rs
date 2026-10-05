//! An interactive session against the server in this process (see `local_server`) and its
//! made-up shell: what NexSSH types into the shell by itself.

mod common;
mod local_server;

use std::sync::Arc;

use common::Session;
use local_server::{LocalServer, Options};
use nexssh_core::SessionStatus;

#[tokio::test(flavor = "multi_thread")]
async fn login_commands_are_typed_once_the_shell_settles() {
    let server = LocalServer::start(Options::default()).await;
    let core = Arc::new(common::core());
    let mut target = server.server();
    target.startup_commands = Some("cd /srv\n\necho hi".into());
    let mut session = Session::open(core, target);
    session.accept_host_key().await;
    session.expect_status(SessionStatus::Connected).await;
    session.wait_output("ran: echo hi").await;

    // After the shell's greeting and prompt, in order, each once; the blank line is not typed.
    let out = session.output.clone();
    let welcome = out.find("Welcome").expect("the greeting");
    let first = out.find("ran: cd /srv").expect("the first command");
    let second = out.find("ran: echo hi").expect("the second command");
    assert!(welcome < first && first < second, "{out}");
    assert_eq!(out.matches("ran:").count(), 2, "{out}");

    // Every login: again after reconnecting.
    session.core.sessions.reconnect(session.id).unwrap();
    session.output.clear();
    session.expect_status(SessionStatus::Connected).await;
    session.wait_output("ran: echo hi").await;
    assert!(
        session.output.contains("ran: cd /srv"),
        "{}",
        session.output
    );
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_is_typed_without_login_commands() {
    let server = LocalServer::start(Options::default()).await;
    let core = Arc::new(common::core());
    let mut session = Session::open(core, server.server());
    session.accept_host_key().await;
    session.expect_status(SessionStatus::Connected).await;
    session.wait_output("$ ").await;
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    session.write("whoami\r");
    session.wait_output("ran: whoami").await;
    assert_eq!(
        session.output.matches("ran:").count(),
        1,
        "{}",
        session.output
    );
    session.close().await;
}
