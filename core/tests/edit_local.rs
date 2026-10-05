//! Remote files edited in a local program (`nexssh_core::edit`), against the server in this
//! process (see `local_server`): saves going back, conflicts, a disconnected session, sudo.

mod common;
mod local_server;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::Session;
use local_server::{LocalServer, Options};
use nexssh_core::SessionStatus;
use nexssh_core::edit::{EditInfo, EditState, Edits, Refusal, Resolution};

async fn connected(server: &LocalServer) -> Session {
    let core = Arc::new(common::core());
    let mut session = Session::open(core, server.server());
    session.accept_host_key().await;
    session.expect_status(SessionStatus::Connected).await;
    session
}

fn edits(session: &Session) -> Arc<Edits> {
    let root = common::temp_dir().join("edit");
    Edits::new(session.core.sessions.clone(), root, Box::new(|_| {}))
}

/// Waits until the edit is in `state` (and, for `Synced`, has been saved since `after`).
async fn until(edits: &Edits, id: u64, state: EditState, after: Option<u64>) -> EditInfo {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let info = edits.get(id).expect("still edited");
        let saved = after.is_none_or(|t| info.saved_at.is_some_and(|s| s > t));
        if info.state == state && saved {
            return info;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never {state:?}: {info:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Saves the local copy as an editor would.
fn save(info: &EditInfo, text: &str) {
    std::fs::write(&info.local_path, text).unwrap();
}

fn read(path: impl AsRef<Path>) -> String {
    String::from_utf8(std::fs::read(path).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn saves_go_back_to_the_server() {
    let server = LocalServer::start(Options::default()).await;
    std::fs::create_dir_all(server.local("/srv")).unwrap();
    std::fs::write(server.local("/srv/app.conf"), "port = 80\n").unwrap();
    let session = connected(&server).await;
    let edits = edits(&session);

    let info = edits.open(session.id, "/srv/app.conf").await.unwrap();
    assert_eq!(info.name, "app.conf");
    assert!(info.local_path.ends_with("app.conf"), "{}", info.local_path);
    assert_eq!(read(&info.local_path), "port = 80\n");
    assert_eq!(info.state, EditState::Synced);
    let again = edits.open(session.id, "/srv/app.conf").await.unwrap();
    assert_eq!(again.id, info.id, "a file edited already keeps its copy");

    save(&info, "port = 8080\n");
    let synced = until(&edits, info.id, EditState::Synced, Some(0)).await;
    assert_eq!(read(server.local("/srv/app.conf")), "port = 8080\n");

    // Same size, another time of change: still noticed.
    save(&info, "port = 8081\n");
    until(&edits, info.id, EditState::Synced, synced.saved_at).await;
    assert_eq!(read(server.local("/srv/app.conf")), "port = 8081\n");

    assert!(edits.open(session.id, "/srv").await.is_err(), "a folder");
    assert!(edits.open(session.id, "/srv/missing").await.is_err());

    let dir = Path::new(&info.local_path).parent().unwrap().to_path_buf();
    edits.stop(info.id);
    assert!(edits.list().is_empty());
    for _ in 0..50 {
        if !dir.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!dir.exists(), "the copy is removed");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_on_the_server_is_not_overwritten() {
    let server = LocalServer::start(Options::default()).await;
    std::fs::write(server.local("/notes.txt"), "mine\n").unwrap();
    let session = connected(&server).await;
    let edits = edits(&session);
    let info = edits.open(session.id, "/notes.txt").await.unwrap();

    // Someone else writes it, then the editor saves.
    std::fs::write(server.local("/notes.txt"), "theirs, longer\n").unwrap();
    save(&info, "edited\n");
    until(&edits, info.id, EditState::Conflict, None).await;
    assert_eq!(read(server.local("/notes.txt")), "theirs, longer\n");

    // Take theirs: the local copy becomes the server's.
    let after = edits.resolve(info.id, Resolution::Reload).await.unwrap();
    assert_eq!(after.state, EditState::Synced);
    assert_eq!(read(&info.local_path), "theirs, longer\n");

    // Again, and this time keep the local copy.
    std::fs::write(server.local("/notes.txt"), "theirs again\n").unwrap();
    save(&info, "mine wins\n");
    until(&edits, info.id, EditState::Conflict, None).await;
    let after = edits.resolve(info.id, Resolution::Overwrite).await.unwrap();
    assert_eq!(after.state, EditState::Synced);
    assert_eq!(read(server.local("/notes.txt")), "mine wins\n");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn saves_wait_for_the_session_to_connect() {
    let server = LocalServer::start(Options::default()).await;
    std::fs::write(server.local("/run.sh"), "echo 1\n").unwrap();
    let mut session = connected(&server).await;
    let edits = edits(&session);
    let info = edits.open(session.id, "/run.sh").await.unwrap();

    session.core.sessions.disconnect(session.id).unwrap();
    session.expect_status(SessionStatus::Disconnected).await;
    save(&info, "echo 2\n");
    until(&edits, info.id, EditState::Waiting, None).await;

    session.core.sessions.reconnect(session.id).unwrap();
    session.expect_status(SessionStatus::Connected).await;
    until(&edits, info.id, EditState::Synced, Some(0)).await;
    assert_eq!(read(server.local("/run.sh")), "echo 2\n");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sudo_reaches_what_sftp_may_not() {
    let server = LocalServer::start(Options {
        locked: Some("/root".into()),
        ..Options::default()
    })
    .await;
    std::fs::create_dir_all(server.local("/root")).unwrap();
    std::fs::write(server.local("/root/.bashrc"), "alias ll='ls -l'\n").unwrap();
    let session = connected(&server).await;
    let edits = edits(&session);

    match edits.open(session.id, "/root/.bashrc").await {
        Err(Refusal::Denied { .. }) => {}
        other => panic!("SFTP may not read it: {other:?}"),
    }
    let info = edits
        .open_sudo(session.id, "/root/.bashrc", None)
        .await
        .unwrap();
    assert!(info.sudo);
    assert_eq!(read(&info.local_path), "alias ll='ls -l'\n");

    save(&info, "alias ll='ls -la'\n");
    until(&edits, info.id, EditState::Synced, Some(0)).await;
    assert_eq!(read(server.local("/root/.bashrc")), "alias ll='ls -la'\n");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sudo_with_a_password() {
    let server = LocalServer::start(Options {
        locked: Some("/etc".into()),
        sudo_password: Some("s3cret".into()),
        ..Options::default()
    })
    .await;
    std::fs::create_dir_all(server.local("/etc")).unwrap();
    std::fs::write(server.local("/etc/hosts"), "127.0.0.1 localhost\n").unwrap();
    let session = connected(&server).await;
    let edits = edits(&session);

    match edits.open_sudo(session.id, "/etc/hosts", None).await {
        Err(Refusal::Password { wrong: false, .. }) => {}
        other => panic!("sudo wants a password: {other:?}"),
    }
    match edits
        .open_sudo(session.id, "/etc/hosts", Some("guess".into()))
        .await
    {
        Err(Refusal::Password { wrong: true, .. }) => {}
        other => panic!("a wrong password: {other:?}"),
    }
    let info = edits
        .open_sudo(session.id, "/etc/hosts", Some("s3cret".into()))
        .await
        .unwrap();
    assert_eq!(read(&info.local_path), "127.0.0.1 localhost\n");

    // The password typed once is used for the saves, and never lands in the file.
    save(&info, "127.0.0.1 localhost\n10.0.0.5 db\n");
    until(&edits, info.id, EditState::Synced, Some(0)).await;
    assert_eq!(
        read(server.local("/etc/hosts")),
        "127.0.0.1 localhost\n10.0.0.5 db\n"
    );
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_sftp_may_not_write_is_saved_with_sudo() {
    let server = LocalServer::start(Options::default()).await;
    let file = server.local("/motd");
    std::fs::write(&file, "hello\n").unwrap();
    let mut permissions = std::fs::metadata(&file).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&file, permissions).unwrap();
    let session = connected(&server).await;
    let edits = edits(&session);

    let info = edits.open(session.id, "/motd").await.unwrap();
    save(&info, "welcome\n");
    let denied = until(&edits, info.id, EditState::Denied, None).await;
    assert!(!denied.needs_password);
    assert_eq!(read(&file), "hello\n");

    let after = edits.use_sudo(info.id, None).await.unwrap();
    assert_eq!(after.state, EditState::Synced, "{after:?}");
    assert!(after.sudo);
    assert_eq!(read(&file), "welcome\n");
    session.close().await;
}
