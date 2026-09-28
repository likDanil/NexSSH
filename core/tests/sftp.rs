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
async fn folders_files_and_permissions() {
    let Some(env) = env() else { return };
    let core = Arc::new(core());
    let mut srv = server(&env, AuthKind::Key);
    srv.identity_file = Some(env.key.clone());
    let mut s = Session::open(Arc::clone(&core), srv);
    s.accept_host_key().await;
    s.expect_status(SessionStatus::Connected).await;
    let sftp = core.sessions.sftp(s.id).await.unwrap();
    let dir = join(
        &sftp.home().await.unwrap(),
        &format!("nexssh-sftp-perm-{}", std::process::id()),
    );
    let _ = sftp.remove(&dir).await;
    let mode_of = |entries: &[nexssh_core::sftp::Entry], name: &str| {
        entries.iter().find(|e| e.name == name).and_then(|e| e.mode)
    };

    // ensure_dir creates once and then merges; a file in the way is an error.
    sftp.ensure_dir(&dir).await.unwrap();
    sftp.ensure_dir(&dir).await.unwrap();
    sftp.ensure_dir(&join(&dir, "sub")).await.unwrap();

    // new_file creates an empty file and never replaces one.
    let file = join(&dir, "sub/run.sh");
    sftp.new_file(&file).await.unwrap();
    let err = sftp.new_file(&file).await.unwrap_err().to_string();
    assert!(err.contains("already exists"), "{err}");
    let err = sftp.ensure_dir(&file).await.unwrap_err().to_string();
    assert!(err.contains("already exists"), "{err}");
    let listed = sftp.list(&join(&dir, "sub")).await.unwrap();
    assert_eq!(listed[0].name, "run.sh");
    assert_eq!(listed[0].size, 0);

    // chmod on one file.
    sftp.chmod(&file, 0o700, false).await.unwrap();
    let listed = sftp.list(&join(&dir, "sub")).await.unwrap();
    assert_eq!(mode_of(&listed, "run.sh"), Some(0o700));
    assert_eq!(listed[0].permissions.as_deref(), Some("rwx------"));

    // Recursively: files get the mode, folders also get x where they have r.
    sftp.chmod(&dir, 0o640, true).await.unwrap();
    let top = sftp.list(&join(&dir, "..")).await.unwrap();
    assert_eq!(
        mode_of(&top, &nexssh_core::sftp::base_name(&dir)),
        Some(0o750)
    );
    let inside = sftp.list(&dir).await.unwrap();
    assert_eq!(mode_of(&inside, "sub"), Some(0o750));
    let listed = sftp.list(&join(&dir, "sub")).await.unwrap();
    assert_eq!(mode_of(&listed, "run.sh"), Some(0o640));

    sftp.remove(&dir).await.unwrap();
    s.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_local_folders() {
    let Some(env) = env() else { return };
    let core = Arc::new(core());
    let mut srv = server(&env, AuthKind::Key);
    srv.identity_file = Some(env.key.clone());
    let mut s = Session::open(Arc::clone(&core), srv);
    s.accept_host_key().await;
    s.expect_status(SessionStatus::Connected).await;
    let sftp = core.sessions.sftp(s.id).await.unwrap();
    let dir = join(
        &sftp.home().await.unwrap(),
        &format!("nexssh-sftp-up-{}", std::process::id()),
    );
    let _ = sftp.remove(&dir).await;
    sftp.mkdir(&dir).await.unwrap();

    // proj/{a.txt, big.bin, src/lib/x.rs, empty/, link.txt -> a.txt, loop -> .}
    let local = local_dir("up");
    let proj = local.join("proj");
    std::fs::create_dir_all(proj.join("src/lib")).unwrap();
    std::fs::create_dir_all(proj.join("empty")).unwrap();
    std::fs::write(proj.join("a.txt"), b"hello").unwrap();
    let big = pattern(700_000);
    std::fs::write(proj.join("big.bin"), &big).unwrap();
    std::fs::write(proj.join("src/lib/x.rs"), b"fn x() {}").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(proj.join("a.txt"), proj.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(&proj, proj.join("loop")).unwrap();
    }

    let cancel = AtomicBool::new(false);
    let mut last = Progress::default();
    let target = sftp
        .upload_path(&proj, &dir, &mut |p| last = p, &cancel)
        .await
        .unwrap();
    assert_eq!(target, join(&dir, "proj"));
    let files = (5 + big.len() + 9) as u64;
    #[cfg(unix)]
    let files = files + 5;
    assert_eq!((last.done, last.total), (files, files));
    let names = |entries: Vec<nexssh_core::sftp::Entry>| {
        let mut names: Vec<_> = entries.into_iter().map(|e| e.name).collect();
        names.sort();
        names
    };
    let mut expected = vec!["a.txt", "big.bin", "empty", "src"];
    if cfg!(unix) {
        expected.push("link.txt");
        expected.sort();
    }
    assert_eq!(names(sftp.list(&target).await.unwrap()), expected);
    assert_eq!(
        names(sftp.list(&join(&target, "src/lib")).await.unwrap()),
        ["x.rs"]
    );
    let copy = sftp
        .download(&join(&target, "big.bin"), &local, &mut |_| {}, &cancel)
        .await
        .unwrap();
    assert_eq!(std::fs::read(copy).unwrap(), big);

    // Again: merges into the folder and replaces the files.
    std::fs::write(proj.join("a.txt"), b"changed").unwrap();
    sftp.upload_path(&proj, &dir, &mut |_| {}, &cancel)
        .await
        .unwrap();
    let a = sftp
        .download(&join(&target, "a.txt"), &local, &mut |_| {}, &cancel)
        .await
        .unwrap();
    assert_eq!(std::fs::read(a).unwrap(), b"changed");

    // A single file; a cancelled upload leaves nothing behind.
    let one = sftp
        .upload_path(&proj.join("big.bin"), &dir, &mut |_| {}, &cancel)
        .await
        .unwrap();
    assert_eq!(one, join(&dir, "big.bin"));
    let cancelled = AtomicBool::new(true);
    let err = sftp
        .upload_path(&proj.join("a.txt"), &dir, &mut |_| {}, &cancelled)
        .await
        .unwrap_err();
    assert!(matches!(err, nexssh_core::Error::Cancelled), "{err}");
    let top = names(sftp.list(&dir).await.unwrap());
    assert_eq!(top, ["big.bin", "proj"]);

    let err = sftp
        .upload_path(&local.join("missing"), &dir, &mut |_| {}, &cancel)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("cannot read"), "{err}");

    sftp.remove(&dir).await.unwrap();
    let _ = std::fs::remove_dir_all(&local);
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
