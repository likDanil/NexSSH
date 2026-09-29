//! Local terminals running real shells in pseudo-terminals. Unix only: Windows' ConPTY
//! is covered by the same code in portable-pty, and tried by hand.
#![cfg(unix)]

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Session, core, temp_dir};
use nexssh_core::{LocalCommand, PtySize, SessionStatus};

/// A plain POSIX shell, so the user's rc files and prompt do not matter.
fn sh(cwd: Option<&Path>) -> LocalCommand {
    let mut command = LocalCommand::parse("/bin/sh", cwd).unwrap();
    command.env.push(("PS1".into(), "$ ".into()));
    command.env.push(("ENV".into(), String::new()));
    command
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[tokio::test(flavor = "multi_thread")]
async fn runs_a_shell_in_a_folder_with_input_output_and_resize() {
    let core = Arc::new(core());
    let dir = temp_dir();
    let mut s = Session::open_local(Arc::clone(&core), sh(Some(&dir)));
    s.expect_status(SessionStatus::Connected).await;

    s.write("echo local-$((40+2))\n");
    s.wait_output("local-42").await;

    s.write("pwd\n");
    s.wait_output(&dir.canonicalize().unwrap().display().to_string())
        .await;

    s.write("echo $TERM_PROGRAM $TERM $COLORTERM\n");
    s.wait_output("NexSSH xterm-256color truecolor").await;

    core.sessions.resize(s.id, PtySize::new(123, 45)).unwrap();
    s.write("stty size\n");
    s.wait_output("45 123").await;

    // A flood of output arrives whole and does not stall the session.
    s.write("seq 1 100000; echo done-$((2+3))\n");
    s.wait_output("done-5").await;
    assert!(s.output.contains("\n99999\r\n100000\r\n"));

    s.close().await;
    assert_eq!(core.sessions.session_count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_folder_starts_in_the_home_folder() {
    let Some(home) = nexssh_core::home_dir().and_then(|h| h.canonicalize().ok()) else {
        return;
    };
    let core = Arc::new(core());
    let mut s = Session::open_local(
        Arc::clone(&core),
        sh(Some(Path::new("/nonexistent/nexssh-folder"))),
    );
    s.expect_status(SessionStatus::Connected).await;
    s.write("pwd -P; echo end-$((3+4))\n");
    s.wait_output("end-7").await;
    assert!(
        s.output.contains(&home.display().to_string()),
        "{}",
        s.output
    );
    s.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn exit_is_reported_and_the_program_can_start_again() {
    let core = Arc::new(core());
    let mut s = Session::open_local(Arc::clone(&core), sh(None));
    s.expect_status(SessionStatus::Connected).await;

    s.write("exit 3\n");
    let (reason, failed) = s.expect_status_full(SessionStatus::Disconnected).await;
    let reason = reason.unwrap();
    assert!(reason.contains("exit code 3"), "{reason}");
    assert!(!failed, "an exit is not a failure");

    // Reconnecting starts the shell again in the same session.
    s.output.clear();
    core.sessions.reconnect(s.id).unwrap();
    s.expect_status(SessionStatus::Connected).await;
    s.write("echo again-$((1+1))\n");
    s.wait_output("again-2").await;

    // Disconnecting stops it and keeps the session.
    core.sessions.disconnect(s.id).unwrap();
    let (reason, failed) = s.expect_status_full(SessionStatus::Disconnected).await;
    assert_eq!(reason.as_deref(), Some("Process stopped"));
    assert!(!failed);
    core.sessions.reconnect(s.id).unwrap();
    s.expect_status(SessionStatus::Connected).await;
    s.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_last_output_of_a_program_arrives_before_its_exit() {
    let core = Arc::new(core());
    let command = LocalCommand::parse("/bin/sh -c 'echo last-words; exit 0'", None).unwrap();
    let mut s = Session::open_local(Arc::clone(&core), command);
    s.expect_status(SessionStatus::Connected).await;
    let (reason, failed) = s.expect_status_full(SessionStatus::Disconnected).await;
    assert!(s.output.contains("last-words"), "{}", s.output);
    assert!(reason.unwrap().contains("exit code 0"));
    assert!(!failed);
    s.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_program_fails_to_start() {
    let core = Arc::new(core());
    let command = LocalCommand::parse("/nonexistent/nexssh-shell -l", None).unwrap();
    let mut s = Session::open_local(Arc::clone(&core), command);
    let (reason, failed) = s.expect_status_full(SessionStatus::Disconnected).await;
    assert!(failed);
    let reason = reason.unwrap();
    assert!(reason.contains("/nonexistent/nexssh-shell"), "{reason}");
    s.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_kills_even_a_program_ignoring_sighup_and_never_enters_the_typed_line() {
    let core = Arc::new(core());
    let dir = temp_dir();
    // Waits for a line with the hang-up signal ignored: closing must neither complete the
    // line (portable-pty's writer sends a newline when dropped) nor leave the program running.
    let command = LocalCommand::parse(
        r#"/bin/sh -c "trap '' HUP; echo $$ > program.pid; echo ready; read line; touch entered""#,
        Some(&dir),
    )
    .unwrap();
    let mut s = Session::open_local(Arc::clone(&core), command);
    s.expect_status(SessionStatus::Connected).await;
    s.wait_output("ready").await;
    let pid = std::fs::read_to_string(dir.join("program.pid")).unwrap();
    let pid = pid.trim();
    assert!(alive(pid));

    s.write("typed-not-entered");
    s.wait_output("typed-not-entered").await;
    s.close().await;

    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        assert!(Instant::now() < deadline, "the program is still running");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!dir.join("entered").exists(), "the typed line was entered");
}
