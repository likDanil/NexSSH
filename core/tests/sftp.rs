//! SFTP against a real OpenSSH server (see `scripts/test-sshd.sh`).

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use common::{Session, core, env, server};
use nexssh_core::sftp::{EntryKind, Progress, Sftp, base_name, join};
use nexssh_core::{AuthKind, SessionStatus};

fn local_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("nexssh-sftp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn upload(sftp: &Sftp, path: &str, data: &[u8]) {
    let mut up = sftp.create(path).await.unwrap();
    for chunk in data.chunks(100_000) {
        up.write(chunk).await.unwrap();
    }
    up.finish().await.unwrap();
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 % 251) as u8).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn browse_upload_download_and_delete() {
    let Some(env) = env() else { return };
    let core = Arc::new(core());
    let mut srv = server(&env, AuthKind::Key);
    srv.identity_file = Some(env.key.clone());
    let mut s = Session::open(Arc::clone(&core), srv);
    s.accept_host_key().await;
    s.expect_status(SessionStatus::Connected).await;

    let sftp = core.sessions.sftp(s.id).await.unwrap();
    let again = core.sessions.sftp(s.id).await.unwrap();
    assert!(
        Arc::ptr_eq(&sftp, &again),
        "one SFTP channel per connection"
    );

    let home = sftp.home().await.unwrap();
    assert!(home.starts_with('/'), "{home}");
    let dir = join(&home, &format!("nexssh-sftp-test-{}", std::process::id()));
    let _ = sftp.remove(&dir).await;
    sftp.mkdir(&dir).await.unwrap();
    let err = sftp.mkdir(&dir).await.unwrap_err().to_string();
    assert!(err.contains("already exists"), "{err}");

    // Upload in chunks, like the UI does.
    let hello = pattern(350_000);
    upload(&sftp, &join(&dir, "hello.txt"), &hello).await;
    sftp.mkdir(&join(&dir, "nested")).await.unwrap();
    assert_eq!(sftp.resolve("~").await.unwrap(), home);
    let typed = format!("~/{}/nested/..", base_name(&dir));
    assert_eq!(sftp.resolve(&typed).await.unwrap(), dir);
    let deep = pattern(1_200_000);
    upload(&sftp, &join(&dir, "nested/deep.bin"), &deep).await;

    let entries = sftp.list(&dir).await.unwrap();
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["nested", "hello.txt"], "folders first");
    assert_eq!(entries[0].kind, EntryKind::Dir);
    assert_eq!(entries[1].size, hello.len() as u64);
    assert!(
        entries[1]
            .permissions
            .as_deref()
            .is_some_and(|p| p.len() == 9)
    );

    // Rename, but never over an existing file.
    sftp.rename(&join(&dir, "hello.txt"), &join(&dir, "greeting.txt"))
        .await
        .unwrap();
    upload(&sftp, &join(&dir, "other.txt"), b"x").await;
    let err = sftp
        .rename(&join(&dir, "other.txt"), &join(&dir, "greeting.txt"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("already exists"), "{err}");

    // Download a file, with progress.
    let local = local_dir("dl");
    let cancel = AtomicBool::new(false);
    let mut last = Progress::default();
    let path = sftp
        .download(
            &join(&dir, "greeting.txt"),
            &local,
            &mut |p| last = p,
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(path, local.join("greeting.txt"));
    assert_eq!(std::fs::read(&path).unwrap(), hello);
    assert_eq!(
        (last.done, last.total),
        (hello.len() as u64, hello.len() as u64)
    );

    // A second download does not overwrite the first.
    let path2 = sftp
        .download(&join(&dir, "greeting.txt"), &local, &mut |_| {}, &cancel)
        .await
        .unwrap();
    assert_eq!(path2, local.join("greeting (1).txt"));

    // A whole directory.
    let copy = sftp
        .download(&dir, &local, &mut |p| last = p, &cancel)
        .await
        .unwrap();
    assert_eq!(std::fs::read(copy.join("greeting.txt")).unwrap(), hello);
    assert_eq!(std::fs::read(copy.join("nested/deep.bin")).unwrap(), deep);
    assert_eq!(last.total, (hello.len() + deep.len() + 1) as u64);
    assert_eq!(last.done, last.total);

    // A cancelled download leaves nothing behind.
    let cancelled = AtomicBool::new(true);
    let err = sftp
        .download(
            &join(&dir, "nested/deep.bin"),
            &local,
            &mut |_| {},
            &cancelled,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, nexssh_core::Error::Cancelled), "{err}");
    assert!(!local.join("deep.bin").exists());

    // Recursive delete.
    sftp.remove(&dir).await.unwrap();
    let err = sftp.list(&dir).await.unwrap_err().to_string();
    assert!(err.contains("no such file"), "{err}");
    assert!(
        !sftp
            .list(&home)
            .await
            .unwrap()
            .iter()
            .any(|e| dir.ends_with(&e.name))
    );

    let _ = std::fs::remove_dir_all(&local);
    assert!(!Path::new(&local).exists());

    // The SFTP client ends with the connection.
    core.sessions.disconnect(s.id).unwrap();
    s.expect_status(SessionStatus::Disconnected).await;
    let err = core.sessions.sftp(s.id).await.err().expect("not connected");
    assert!(err.to_string().contains("not connected"), "{err}");
    s.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn permission_errors_are_readable() {
    let Some(env) = env() else { return };
    let core = Arc::new(core());
    let mut srv = server(&env, AuthKind::Key);
    srv.identity_file = Some(env.key.clone());
    let mut s = Session::open(Arc::clone(&core), srv);
    s.accept_host_key().await;
    s.expect_status(SessionStatus::Connected).await;

    let sftp = core.sessions.sftp(s.id).await.unwrap();
    let err = sftp.list("/root").await.unwrap_err().to_string();
    assert!(
        err.contains("/root") && err.contains("permission denied"),
        "{err}"
    );
    let err = sftp
        .mkdir("/nexssh-not-allowed")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("permission denied"), "{err}");
    s.close().await;
}
