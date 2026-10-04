//! Commands run on a session's connection without a terminal (`SessionManager::exec`),
//! against the server in this process (see `local_server`): they run anywhere, no sshd
//! needed.

mod common;
mod local_server;

use std::sync::Arc;
use std::time::Duration;

use common::Session;
use local_server::{LocalServer, Options};
use nexssh_core::SessionStatus;
use nexssh_core::exec::{ExecOptions, ExecOutput, Exit};

fn stdout(output: &ExecOutput) -> String {
    let (head, tail) = output.stdout.parts();
    format!(
        "{}{}",
        String::from_utf8_lossy(head),
        String::from_utf8_lossy(&tail)
    )
}

fn stderr(output: &ExecOutput) -> String {
    let (head, tail) = output.stderr.parts();
    format!(
        "{}{}",
        String::from_utf8_lossy(head),
        String::from_utf8_lossy(&tail)
    )
}

async fn connected(server: &LocalServer, id: &str) -> Session {
    let core = Arc::new(common::core());
    let mut target = server.server();
    target.id = id.into();
    let mut session = Session::open(core, target);
    session.accept_host_key().await;
    session.expect_status(SessionStatus::Connected).await;
    session
}

#[tokio::test(flavor = "multi_thread")]
async fn runs_commands_next_to_the_terminal() {
    let server = LocalServer::start(Options::default()).await;
    let session = connected(&server, "").await;
    let sessions = &session.core.sessions;

    let out = sessions
        .exec(session.id, "echo hello there", ExecOptions::default())
        .await
        .unwrap();
    assert_eq!(stdout(&out), "hello there\n");
    assert_eq!(stderr(&out), "");
    assert_eq!(out.exit, Some(Exit::Code(0)));
    assert!(!out.timed_out);

    let out = sessions
        .exec(session.id, "fail", ExecOptions::default())
        .await
        .unwrap();
    assert_eq!(stderr(&out), "boom\n");
    assert_eq!(out.exit, Some(Exit::Code(3)));

    // Messages may come before the server answers the request (OpenSSH widens the window).
    let out = sessions
        .exec(session.id, "early", ExecOptions::default())
        .await
        .unwrap();
    assert_eq!(stdout(&out), "early\n");
    assert_eq!(out.exit, Some(Exit::Code(0)));

    // Several at once, each on a channel of its own.
    let runs = (0..8).map(|i| {
        let sessions = sessions.clone();
        let id = session.id;
        tokio::spawn(async move {
            let command = format!("echo {i}");
            sessions.exec(id, &command, ExecOptions::default()).await
        })
    });
    for (i, run) in runs.enumerate() {
        let out = run.await.unwrap().unwrap();
        assert_eq!(stdout(&out), format!("{i}\n"));
    }
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn gives_input_then_ends_it() {
    let server = LocalServer::start(Options::default()).await;
    let session = connected(&server, "").await;
    let sessions = &session.core.sessions;

    let input = "line one\nline two\n".repeat(20_000);
    let options = ExecOptions {
        stdin: input.clone().into_bytes(),
        max_output: 1024 * 1024,
        ..ExecOptions::default()
    };
    let out = sessions.exec(session.id, "cat", options).await.unwrap();
    assert_eq!(stdout(&out), input);

    // Without input the command sees its end at once instead of waiting.
    let out = sessions
        .exec(session.id, "cat", ExecOptions::default())
        .await
        .unwrap();
    assert_eq!(out.stdout.total(), 0);
    assert_eq!(out.exit, Some(Exit::Code(0)));
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn keeps_the_beginning_and_the_end_of_long_output() {
    let server = LocalServer::start(Options::default()).await;
    let session = connected(&server, "").await;
    let options = ExecOptions {
        max_output: 1000,
        ..ExecOptions::default()
    };
    let out = session
        .core
        .sessions
        .exec(session.id, "flood 3000000", options)
        .await
        .unwrap();
    assert_eq!(out.stdout.total(), 3_000_000);
    assert_eq!(out.stdout.omitted(), 2_999_000);
    let (head, tail) = out.stdout.parts();
    assert_eq!(head.len(), 250);
    assert!(head.starts_with(b"0123456789"));
    assert_eq!(tail.len(), 750);
    assert!(tail.ends_with(b"789"), "the very last digits");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stops_a_command_that_takes_too_long() {
    let server = LocalServer::start(Options::default()).await;
    let session = connected(&server, "").await;
    let options = ExecOptions {
        timeout: Duration::from_millis(300),
        ..ExecOptions::default()
    };
    let out = session
        .core
        .sessions
        .exec(session.id, "hang", options)
        .await
        .unwrap();
    assert!(out.timed_out);
    assert_eq!(out.exit, None);

    // The connection is fine afterwards.
    let out = session
        .core
        .sessions
        .exec(session.id, "echo still here", ExecOptions::default())
        .await
        .unwrap();
    assert_eq!(stdout(&out), "still here\n");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn finds_the_connected_session_of_a_saved_server() {
    let server = LocalServer::start(Options::default()).await;
    let core = Arc::new(common::core());
    let mut changes = core.sessions.live_changes();
    assert_eq!(core.sessions.connected("web"), None);

    let mut target = server.server();
    target.id = "web".into();
    let mut session = Session::open(Arc::clone(&core), target);
    session.accept_host_key().await;
    session.expect_status(SessionStatus::Connected).await;
    assert!(changes.has_changed().unwrap(), "connecting is announced");
    changes.mark_unchanged();
    assert_eq!(core.sessions.connected("web"), Some(session.id));
    assert_eq!(core.sessions.connected("other"), None);
    assert_eq!(
        core.sessions.connected(""),
        None,
        "quick connections have no id"
    );

    core.sessions.disconnect(session.id).unwrap();
    session.expect_status(SessionStatus::Disconnected).await;
    assert!(
        changes.has_changed().unwrap(),
        "so is the end of the connection"
    );
    assert_eq!(core.sessions.connected("web"), None);
    let refused = core
        .sessions
        .exec(session.id, "echo no", ExecOptions::default())
        .await;
    assert!(refused.is_err(), "no connection, no commands");
    session.close().await;
}
