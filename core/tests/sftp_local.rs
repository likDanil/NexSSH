//! SFTP transfers against a server in this process (see `local_server`), behind a proxy that
//! adds latency: they run anywhere, no sshd needed.
//!
//! Transfer speeds (one-way delay in ms, size in MiB, number of small files, optional cap in
//! Mbit/s):
//!
//!     NEXSSH_BENCH_DELAY_MS=25 NEXSSH_BENCH_MB=64 cargo test --release -p nexssh-core \
//!         --test sftp_local transfer_speed -- --ignored --nocapture
//!
//! A server like it to try the app against, until stopped (same delay and cap variables):
//!
//!     cargo test -p nexssh-core --test sftp_local serve -- --ignored --nocapture

mod common;
mod local_server;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use local_server::{LocalServer, Options};
use nexssh_core::sftp::Progress;

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn pattern(len: usize, seed: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 7 + seed) % 251) as u8).collect()
}

fn no_progress() -> impl FnMut(Progress) + Send {
    |_| {}
}

fn rate(bytes: u64, took: Duration) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    format!(
        "{mb:7.1} MiB in {:6.2} s = {:7.2} MiB/s",
        took.as_secs_f64(),
        mb / took.as_secs_f64()
    )
}

fn files_rate(count: u64, took: Duration) -> String {
    format!(
        "{count:5} files in {:6.2} s = {:7.1} files/s",
        took.as_secs_f64(),
        count as f64 / took.as_secs_f64()
    )
}

fn make_tree(dir: &Path, files: u64, size: usize) {
    for i in 0..files {
        let sub = dir.join(format!("d{}", i % 10));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(format!("f{i}.txt")), pattern(size, i as usize)).unwrap();
    }
}

/// Everything under `root`: folders (as `None`) and files with their contents.
fn tree(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut found = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for item in std::fs::read_dir(&dir).unwrap() {
            let path = item.unwrap().path();
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                found.insert(name, None);
                pending.push(path);
            } else {
                found.insert(name, Some(std::fs::read(&path).unwrap()));
            }
        }
    }
    found
}

/// proj/{a.txt, empty.txt, deep/er/b.bin, nothing/, many/f0..f149.txt}
fn make_project(local: &Path) -> std::path::PathBuf {
    let proj = local.join("proj");
    std::fs::create_dir_all(proj.join("deep/er")).unwrap();
    std::fs::create_dir_all(proj.join("nothing")).unwrap();
    std::fs::create_dir_all(proj.join("many")).unwrap();
    std::fs::write(proj.join("a.txt"), b"hello").unwrap();
    std::fs::write(proj.join("empty.txt"), b"").unwrap();
    std::fs::write(proj.join("deep/er/b.bin"), pattern(300_001, 2)).unwrap();
    for i in 0..150 {
        std::fs::write(proj.join(format!("many/f{i}.txt")), pattern(i * 37, i)).unwrap();
    }
    proj
}

/// The names in a folder, hidden ones too, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|item| item.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Sets `cancel` after a moment, while a transfer is on its way.
async fn cancel_soon(cancel: &AtomicBool) {
    tokio::time::sleep(Duration::from_millis(700)).await;
    cancel.store(true, Ordering::Relaxed);
}

#[tokio::test(flavor = "multi_thread")]
async fn files_and_folders_over_a_slow_link() {
    let server = LocalServer::start(Options {
        delay: Duration::from_millis(10),
        ..Options::default()
    })
    .await;
    let (_session, sftp) = server.connect().await;
    let cancel = AtomicBool::new(false);
    let local = common::temp_dir();
    let down = local.join("down");

    // A file of an odd size, up and down, with progress.
    let data = pattern(3 * 1024 * 1024 + 12_345, 3);
    let len = data.len() as u64;
    let src = local.join("src.bin");
    std::fs::write(&src, &data).unwrap();
    let mut last = Progress::default();
    let remote = sftp
        .upload_path(&src, "/", &mut |p| last = p, &cancel)
        .await
        .unwrap();
    assert_eq!(remote, "/src.bin");
    assert_eq!((last.done, last.total), (len, len));
    assert_eq!(std::fs::read(server.local("/src.bin")).unwrap(), data);
    let got = sftp
        .download("/src.bin", &down, &mut |p| last = p, &cancel)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&got).unwrap(), data);
    assert_eq!((last.done, last.total), (len, len));

    // Pieces of odd sizes, as the page sends them.
    let mut upload = sftp.create("/pieces.bin").await.unwrap();
    for piece in data.chunks(100_003) {
        upload.write(piece).await.unwrap();
    }
    upload.finish().await.unwrap();
    assert_eq!(std::fs::read(server.local("/pieces.bin")).unwrap(), data);

    // Empty files.
    std::fs::write(local.join("nothing.txt"), b"").unwrap();
    sftp.upload_path(&local.join("nothing.txt"), "/", &mut |_| {}, &cancel)
        .await
        .unwrap();
    assert_eq!(std::fs::read(server.local("/nothing.txt")).unwrap(), b"");
    let got = sftp
        .download("/nothing.txt", &down, &mut |_| {}, &cancel)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&got).unwrap(), b"");

    // A folder with nested and empty folders and more files than one listing reply holds.
    let proj = make_project(&local);
    let target = sftp
        .upload_path(&proj, "/", &mut |p| last = p, &cancel)
        .await
        .unwrap();
    assert_eq!(target, "/proj");
    assert_eq!(last.done, last.total);
    assert_eq!(tree(&server.local("/proj")), tree(&proj));
    assert_eq!(sftp.list("/proj/many").await.unwrap().len(), 150);
    let copy = sftp
        .download("/proj", &down, &mut |p| last = p, &cancel)
        .await
        .unwrap();
    assert_eq!(tree(&copy), tree(&proj));
    assert_eq!(last.done, last.total);

    // Uploading again merges into the folder and replaces its files.
    std::fs::write(proj.join("a.txt"), b"changed").unwrap();
    sftp.upload_path(&proj, "/", &mut |_| {}, &cancel)
        .await
        .unwrap();
    assert_eq!(tree(&server.local("/proj")), tree(&proj));

    // Several downloads at once on the one channel, as the drawer starts them.
    let (mut pa, mut pb, mut pc) = (no_progress(), no_progress(), no_progress());
    let (a, b, c) = tokio::join!(
        sftp.download("/src.bin", &down, &mut pa, &cancel),
        sftp.download("/proj", &down, &mut pb, &cancel),
        sftp.download("/pieces.bin", &down, &mut pc, &cancel),
    );
    assert_eq!(std::fs::read(a.unwrap()).unwrap(), data);
    assert_eq!(tree(&b.unwrap()), tree(&proj));
    assert_eq!(std::fs::read(c.unwrap()).unwrap(), data);

    // Folders for an upload: parents first, existing ones kept, a file in the way refused.
    let dirs = ["/n/a/b", "/n", "/n/a", "/proj/deep"].map(String::from);
    sftp.ensure_dirs(&dirs).await.unwrap();
    assert!(server.local("/n/a/b").is_dir());
    let err = sftp
        .ensure_dirs(&["/proj/a.txt".to_string()])
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("already exists"), "{err}");

    sftp.chmod("/proj", 0o640, true).await.unwrap();
    sftp.remove("/proj").await.unwrap();
    assert!(!server.local("/proj").exists());
    let _ = std::fs::remove_dir_all(&local);
}

#[tokio::test(flavor = "multi_thread")]
async fn servers_that_read_less_or_hide_sizes() {
    let servers = [
        // Without limits@openssh.com, and reading less than asked for.
        Options {
            max_read: 20_000,
            ..Options::default().old_openssh()
        },
        // Every file says it is empty, like /proc files.
        Options {
            hide_sizes: true,
            ..Options::default()
        },
    ];
    for options in servers {
        let server = LocalServer::start(options.clone()).await;
        let (_session, sftp) = server.connect().await;
        let cancel = AtomicBool::new(false);
        let local = common::temp_dir();
        let data = pattern(1024 * 1024 + 777, 5);
        std::fs::write(server.local("/f.bin"), &data).unwrap();
        let got = sftp
            .download("/f.bin", &local, &mut |_| {}, &cancel)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&got).unwrap(), data, "{options:?}");

        let proj = make_project(&local);
        sftp.upload_path(&proj, "/", &mut |_| {}, &cancel)
            .await
            .unwrap();
        assert_eq!(tree(&server.local("/proj")), tree(&proj), "{options:?}");
        let copy = sftp
            .download("/proj", &local.join("down"), &mut |_| {}, &cancel)
            .await
            .unwrap();
        assert_eq!(tree(&copy), tree(&proj), "{options:?}");
        let _ = std::fs::remove_dir_all(&local);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn small_files_in_memory() {
    let servers = [
        Options::default(),
        // Reads shorter than asked for, sizes hidden like /proc files.
        Options {
            max_read: 20_000,
            hide_sizes: true,
            ..Options::default().old_openssh()
        },
    ];
    for options in servers {
        let server = LocalServer::start(options.clone()).await;
        let (_session, sftp) = server.connect().await;
        let data = pattern(300_000, 5);
        std::fs::write(server.local("/notes.txt"), &data).unwrap();

        let (whole, size) = sftp.read_part("/notes.txt", 0, 1_000_000).await.unwrap();
        assert!(whole == data, "{options:?}");
        let said = if options.hide_sizes { 0 } else { 300_000 };
        assert_eq!(size, Some(said));
        let (part, _) = sftp.read_part("/notes.txt", 299_990, 100).await.unwrap();
        assert_eq!(part, data[299_990..]);
        let (part, _) = sftp.read_part("/notes.txt", 1000, 50).await.unwrap();
        assert_eq!(part, data[1000..1050]);
        assert!(sftp.read_part("/", 0, 10).await.is_err(), "a folder");
        assert!(sftp.read_part("/missing", 0, 10).await.is_err());

        // Written whole: shorter content replaces longer, and new files are created.
        let text = pattern(600_000, 1);
        sftp.write_whole("/notes.txt", &text).await.unwrap();
        assert!(std::fs::read(server.local("/notes.txt")).unwrap() == text);
        sftp.write_whole("/notes.txt", b"short").await.unwrap();
        assert_eq!(std::fs::read(server.local("/notes.txt")).unwrap(), b"short");
        sftp.write_whole("/empty", b"").await.unwrap();
        assert_eq!(std::fs::read(server.local("/empty")).unwrap(), b"");
        assert!(sftp.write_whole("/no/such/dir/file", b"x").await.is_err());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_transfers_leave_nothing_behind() {
    let server = LocalServer::start(Options {
        delay: Duration::from_millis(10),
        bandwidth: Some(4 * 1024 * 1024),
        ..Options::default()
    })
    .await;
    let (_session, sftp) = server.connect().await;
    let local = common::temp_dir();
    let data = pattern(16 * 1024 * 1024, 9);
    std::fs::write(server.local("/big.bin"), &data).unwrap();

    let cancel = AtomicBool::new(false);
    let mut seen = 0;
    let mut progress = |p: Progress| seen = p.done;
    let down = local.join("down");
    let (result, ()) = tokio::join!(
        sftp.download("/big.bin", &down, &mut progress, &cancel),
        cancel_soon(&cancel)
    );
    assert!(
        matches!(result, Err(nexssh_core::Error::Cancelled)),
        "{result:?}"
    );
    assert!(seen > 0 && seen < data.len() as u64, "{seen}");
    assert!(std::fs::read_dir(&down).unwrap().next().is_none());

    // A folder: every file on its way is removed; whatever is left is complete.
    let proj = local.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    for i in 0..4 {
        std::fs::write(proj.join(format!("{i}.bin")), &data[..4 * 1024 * 1024]).unwrap();
    }
    cancel.store(false, Ordering::Relaxed);
    let mut progress = no_progress();
    let (result, ()) = tokio::join!(
        sftp.upload_path(&proj, "/", &mut progress, &cancel),
        cancel_soon(&cancel)
    );
    assert!(
        matches!(result, Err(nexssh_core::Error::Cancelled)),
        "{result:?}"
    );
    // The removals run before the upload returns.
    for (name, content) in tree(&server.local("/proj")) {
        assert_eq!(content.as_deref(), Some(&data[..4 * 1024 * 1024]), "{name}");
    }
    let _ = std::fs::remove_dir_all(&local);
}

#[tokio::test(flavor = "multi_thread")]
async fn replaced_files_stay_until_the_upload_is_complete() {
    // OpenSSH replaces in one rename; without that, the original moves aside first.
    for posix_rename in [true, false] {
        let server = LocalServer::start(Options {
            delay: Duration::from_millis(10),
            bandwidth: Some(4 * 1024 * 1024),
            posix_rename,
            ..Options::default()
        })
        .await;
        let (_session, sftp) = server.connect().await;
        let local = common::temp_dir();
        let old = pattern(100_000, 1);
        let new = pattern(8 * 1024 * 1024, 2);
        std::fs::write(server.local("/keep.bin"), &old).unwrap();
        let src = local.join("keep.bin");
        std::fs::write(&src, &new).unwrap();
        let on_server = || std::fs::read(server.local("/keep.bin")).unwrap();

        // Cancelled on its way: the file is as it was, and no copy is left beside it.
        let cancel = AtomicBool::new(false);
        let mut seen = 0;
        let mut progress = |p: Progress| seen = p.done;
        let (result, ()) = tokio::join!(
            sftp.upload_path(&src, "/", &mut progress, &cancel),
            cancel_soon(&cancel)
        );
        assert!(
            matches!(result, Err(nexssh_core::Error::Cancelled)),
            "{result:?}"
        );
        assert!(seen > 0 && seen < new.len() as u64, "{seen}");
        assert!(on_server() == old, "posix-rename {posix_rename}");
        assert_eq!(names(&server.root), ["keep.bin"]);

        // Pieces from the page, abandoned.
        let mut upload = sftp.create("/keep.bin").await.unwrap();
        upload.write(&new[..1024 * 1024]).await.unwrap();
        upload.cancel().await;
        assert!(on_server() == old);
        assert_eq!(names(&server.root), ["keep.bin"]);

        // Complete: replaced, and nothing else left.
        cancel.store(false, Ordering::Relaxed);
        sftp.upload_path(&src, "/", &mut no_progress(), &cancel)
            .await
            .unwrap();
        assert!(on_server() == new);
        let mut upload = sftp.create("/keep.bin").await.unwrap();
        upload.write(&old).await.unwrap();
        upload.finish().await.unwrap();
        assert!(on_server() == old);
        assert_eq!(names(&server.root), ["keep.bin"]);

        // A file this user may not write is not replaced.
        let file = server.local("/keep.bin");
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions.clone()).unwrap();
        let err = sftp
            .upload_path(&src, "/", &mut no_progress(), &cancel)
            .await
            .unwrap_err();
        assert!(matches!(err, nexssh_core::Error::Denied(_)), "{err}");
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&file, permissions).unwrap();
        assert!(on_server() == old);
        assert_eq!(names(&server.root), ["keep.bin"]);

        // Nor is a folder.
        std::fs::create_dir(server.local("/dir.bin")).unwrap();
        let err = sftp.create("/dir.bin").await.err().expect("a folder");
        assert!(err.to_string().contains("is a folder"), "{err}");
        std::fs::remove_dir(server.local("/dir.bin")).unwrap();

        // A folder whose files replace others, cancelled: each file is the old one or the new
        // one, never part of one or gone, and no copies are left.
        let proj = local.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(server.local("/proj")).unwrap();
        let part = &new[..3 * 1024 * 1024];
        for i in 0..4 {
            std::fs::write(proj.join(format!("{i}.bin")), part).unwrap();
            std::fs::write(server.local(&format!("/proj/{i}.bin")), &old).unwrap();
        }
        cancel.store(false, Ordering::Relaxed);
        let mut progress = no_progress();
        let (result, ()) = tokio::join!(
            sftp.upload_path(&proj, "/", &mut progress, &cancel),
            cancel_soon(&cancel)
        );
        assert!(
            matches!(result, Err(nexssh_core::Error::Cancelled)),
            "{result:?}"
        );
        let after = tree(&server.local("/proj"));
        assert_eq!(after.len(), 4, "{:?}", after.keys());
        for (name, content) in after {
            let content = content.unwrap();
            assert!(content == old || content == part, "{name}");
        }

        // Complete, with a file the folder did not have: all there, and nothing else.
        std::fs::write(proj.join("added.txt"), b"added").unwrap();
        cancel.store(false, Ordering::Relaxed);
        sftp.upload_path(&proj, "/", &mut no_progress(), &cancel)
            .await
            .unwrap();
        assert_eq!(tree(&server.local("/proj")), tree(&proj));

        // A file in it this user may not write stops the upload and stays as it was.
        let file = server.local("/proj/0.bin");
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions.clone()).unwrap();
        std::fs::write(proj.join("0.bin"), &old).unwrap();
        let err = sftp
            .upload_path(&proj, "/", &mut no_progress(), &cancel)
            .await
            .unwrap_err();
        assert!(matches!(err, nexssh_core::Error::Denied(_)), "{err}");
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&file, permissions).unwrap();
        assert!(std::fs::read(&file).unwrap() == part);
        let expected = ["0.bin", "1.bin", "2.bin", "3.bin", "added.txt"];
        assert_eq!(names(&server.local("/proj")), expected);
        let _ = std::fs::remove_dir_all(&local);
    }
}

/// The link from `NEXSSH_BENCH_DELAY_MS` (one way) and `NEXSSH_BENCH_BW_MBIT`.
fn bench_link() -> Options {
    Options {
        delay: Duration::from_millis(env_u64("NEXSSH_BENCH_DELAY_MS", 25)),
        bandwidth: std::env::var("NEXSSH_BENCH_BW_MBIT")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|mbit| mbit * 1_000_000 / 8),
        ..Options::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "serves until stopped; run with --ignored --nocapture"]
async fn serve() {
    let server = LocalServer::start(bench_link()).await;
    println!(
        "SFTP on 127.0.0.1:{} (any user), key {}, files in {}",
        server.port,
        server.key_file.display(),
        server.root.display()
    );
    std::future::pending::<()>().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "measures transfer speed; run with --ignored --nocapture"]
async fn transfer_speed() {
    let size = env_u64("NEXSSH_BENCH_MB", 64) as usize * 1024 * 1024;
    let small = env_u64("NEXSSH_BENCH_FILES", 300);
    let link = bench_link();
    let bandwidth = link.bandwidth;
    let server = LocalServer::start(link).await;
    let (_session, sftp) = server.connect().await;
    let cancel = AtomicBool::new(false);

    let started = Instant::now();
    for _ in 0..5 {
        sftp.resolve(".").await.unwrap();
    }
    println!(
        "round trip {:.1} ms, bandwidth {}",
        started.elapsed().as_secs_f64() * 1000.0 / 5.0,
        bandwidth.map_or("unlimited".into(), |b| format!(
            "{} Mbit/s",
            b * 8 / 1_000_000
        ))
    );

    let data = pattern(size, 1);
    std::fs::write(server.local("/big.bin"), &data).unwrap();
    let local = common::temp_dir();

    let started = Instant::now();
    let got = sftp
        .download("/big.bin", &local, &mut no_progress(), &cancel)
        .await
        .unwrap();
    println!("download          {}", rate(size as u64, started.elapsed()));
    assert_eq!(std::fs::metadata(&got).unwrap().len(), size as u64);

    std::fs::create_dir_all(server.local("/up")).unwrap();
    let started = Instant::now();
    sftp.upload_path(&got, "/up", &mut no_progress(), &cancel)
        .await
        .unwrap();
    println!("upload from disk  {}", rate(size as u64, started.elapsed()));
    assert_eq!(std::fs::read(server.local("/up/big.bin")).unwrap(), data);

    let started = Instant::now();
    sftp.upload_path(&got, "/up", &mut no_progress(), &cancel)
        .await
        .unwrap();
    println!("replace a file    {}", rate(size as u64, started.elapsed()));

    let started = Instant::now();
    let mut upload = sftp.create("/up/pieces.bin").await.unwrap();
    for piece in data.chunks(1024 * 1024) {
        upload.write(piece).await.unwrap();
    }
    upload.finish().await.unwrap();
    println!("upload in pieces  {}", rate(size as u64, started.elapsed()));
    assert_eq!(std::fs::read(server.local("/up/pieces.bin")).unwrap(), data);

    make_tree(&server.local("/tree"), small, 8 * 1024);
    let started = Instant::now();
    let tree = sftp
        .download("/tree", &local, &mut no_progress(), &cancel)
        .await
        .unwrap();
    println!("download folder   {}", files_rate(small, started.elapsed()));

    let started = Instant::now();
    sftp.upload_path(&tree, "/up", &mut no_progress(), &cancel)
        .await
        .unwrap();
    println!("upload folder     {}", files_rate(small, started.elapsed()));

    let started = Instant::now();
    sftp.upload_path(&tree, "/up", &mut no_progress(), &cancel)
        .await
        .unwrap();
    println!("replace folder    {}", files_rate(small, started.elapsed()));
    let _ = std::fs::remove_dir_all(&local);
}
