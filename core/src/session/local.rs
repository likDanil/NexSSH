//! A local terminal: a program on this computer in a pseudo-terminal (ConPTY on Windows),
//! driven through the same commands and events as an SSH session. Reconnecting starts the
//! program again in the same tab; disconnecting stops it.
//!
//! The pty API is blocking, so each running program has three threads: one reads its
//! output, one writes its input, one waits for it to exit.

use std::io::{ErrorKind, Read, Write};
use std::sync::Arc;
use std::time::Duration;

use portable_pty::{ChildKiller, CommandBuilder, ExitStatus, MasterPty, native_pty_system};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::shell::{Control, FLUSH_BYTES, FLUSH_INTERVAL, idle};
use super::{Command, LogLevel, SessionCtx, SessionStatus, lock};
use crate::error::{Error, Result};
use crate::i18n;
use crate::local::{LocalCommand, find_program};
use crate::model::PtySize;

/// Chunks of output on their way to the session task. While the queue is full the reader
/// waits, so a flood of output stays in the terminal instead of filling memory.
const OUTPUT_QUEUE: usize = 64;
/// How long to wait for the rest of the output once the program has exited (or for the
/// program to exit once its terminal has closed).
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
/// A program still running this long after the hang-up signal is killed (Unix).
#[cfg(unix)]
const KILL_GRACE: Duration = Duration::from_secs(1);

enum Outcome {
    Closed,
    Restart,
    Ended(String),
    Failed(String),
}

pub(crate) async fn run(
    ctx: Arc<SessionCtx>,
    command: LocalCommand,
    mut size: PtySize,
    mut rx: mpsc::UnboundedReceiver<Command>,
) {
    loop {
        ctx.status(SessionStatus::Connecting);
        let started = {
            let (command, size) = (command.clone(), size);
            tokio::task::spawn_blocking(move || Pty::start(&command, size)).await
        };
        let outcome = match started {
            Ok(Ok(pty)) => pty.run(&ctx, &mut size, &mut rx).await,
            Ok(Err(e)) => {
                ctx.log(LogLevel::Error, e.to_string());
                Outcome::Failed(e.to_string())
            }
            Err(e) => Outcome::Failed(e.to_string()),
        };
        let (reason, failed) = match outcome {
            Outcome::Closed => break,
            Outcome::Restart => continue,
            Outcome::Ended(reason) => (reason, false),
            Outcome::Failed(reason) => (reason, true),
        };
        ctx.disconnected(reason, failed);
        match idle(&mut rx, &mut size, false).await {
            Control::Close => break,
            Control::Reconnect | Control::Stop => continue,
        }
    }
    lock(&ctx.shared.sessions).remove(&ctx.id);
    ctx.status(SessionStatus::Closed);
}

/// A running program in a pseudo-terminal.
struct Pty {
    /// Taken when the terminal is closed.
    master: Option<Box<dyn MasterPty + Send>>,
    /// Sends SIGHUP on Unix, terminates the process on Windows.
    killer: Box<dyn ChildKiller + Send + Sync>,
    #[cfg(unix)]
    pid: Option<u32>,
    output: mpsc::Receiver<Vec<u8>>,
    /// The exit status (`None` if it could not be read).
    exited: oneshot::Receiver<Option<ExitStatus>>,
    input: std::sync::mpsc::Sender<Vec<u8>>,
}

impl Pty {
    fn start(command: &LocalCommand, size: PtySize) -> Result<Pty> {
        let name = command.program.display().to_string();
        let program = find_program(&command.program)
            .ok_or_else(|| Error::NotFound(i18n::program_not_found(&name)))?;
        let failed = |e: &dyn std::fmt::Display| Error::invalid(i18n::local_start_failed(&name, e));

        let pair = native_pty_system()
            .openpty(native_size(size))
            .map_err(|e| failed(&e))?;
        let mut builder = CommandBuilder::new(&program);
        builder.args(&command.args);
        if let Some(cwd) = &command.cwd {
            builder.cwd(cwd);
        }
        for (key, value) in &command.env {
            builder.env(key, value);
        }
        let mut child = pair.slave.spawn_command(builder).map_err(|e| failed(&e))?;
        // The program has its own handle on the terminal; ours would keep the terminal open
        // after the program exits, and its output would never end.
        drop(pair.slave);
        let mut killer = child.clone_killer();
        #[cfg(unix)]
        let pid = child.process_id();

        let threads = (|| -> std::io::Result<_> {
            let reader = pair
                .master
                .try_clone_reader()
                .map_err(std::io::Error::other)?;
            let writer = writer(pair.master.as_ref())?;
            let (output_tx, output) = mpsc::channel(OUTPUT_QUEUE);
            thread("pty-read", move || read_output(reader, output_tx))?;
            let (input, input_rx) = std::sync::mpsc::channel();
            thread("pty-write", move || write_input(writer, input_rx))?;
            let (exit_tx, exited) = oneshot::channel();
            thread("pty-wait", move || {
                let _ = exit_tx.send(child.wait().ok());
            })?;
            Ok((output, input, exited))
        })();
        match threads {
            Ok((output, input, exited)) => Ok(Pty {
                master: Some(pair.master),
                killer,
                #[cfg(unix)]
                pid,
                output,
                exited,
                input,
            }),
            Err(e) => {
                let _ = killer.kill();
                Err(failed(&e))
            }
        }
    }

    async fn run(
        mut self,
        ctx: &Arc<SessionCtx>,
        size: &mut PtySize,
        rx: &mut mpsc::UnboundedReceiver<Command>,
    ) -> Outcome {
        ctx.status(SessionStatus::Connected);

        // Output is coalesced like an SSH session's (see `shell.rs`).
        let mut out: Vec<u8> = Vec::new();
        let mut last_flush = Instant::now() - FLUSH_INTERVAL;
        let flush_timer = tokio::time::sleep(Duration::from_secs(3600));
        tokio::pin!(flush_timer);
        let mut armed = false;

        // The end: the output ran out (`eof`) and the program exited (`exit`), or one of
        // them happened and the other did not follow within `DRAIN_TIMEOUT`.
        let mut eof = false;
        let mut exit: Option<Option<ExitStatus>> = None;
        let drain = tokio::time::sleep(Duration::from_secs(3600));
        tokio::pin!(drain);

        let requested = loop {
            if eof && exit.is_some() {
                break None;
            }
            tokio::select! {
                chunk = self.output.recv(), if !eof => match chunk {
                    Some(data) => {
                        out.extend_from_slice(&data);
                        if out.len() >= FLUSH_BYTES || (!armed && last_flush.elapsed() >= FLUSH_INTERVAL) {
                            ctx.sink.output(std::mem::take(&mut out));
                            last_flush = Instant::now();
                            armed = false;
                        } else if !armed {
                            flush_timer.as_mut().reset(last_flush + FLUSH_INTERVAL);
                            armed = true;
                        }
                    }
                    None => {
                        eof = true;
                        if exit.is_none() {
                            drain.as_mut().reset(Instant::now() + DRAIN_TIMEOUT);
                        }
                    }
                },
                status = &mut self.exited, if exit.is_none() => {
                    exit = Some(status.ok().flatten());
                    // ConPTY hands over the last output, and ends it, once it is closed.
                    self.release();
                    if !eof {
                        drain.as_mut().reset(Instant::now() + DRAIN_TIMEOUT);
                    }
                }
                () = &mut flush_timer, if armed => {
                    if !out.is_empty() {
                        ctx.sink.output(std::mem::take(&mut out));
                    }
                    last_flush = Instant::now();
                    armed = false;
                }
                () = &mut drain, if eof || exit.is_some() => break None,
                cmd = rx.recv() => match cmd {
                    Some(Command::Write(bytes)) => {
                        let _ = self.input.send(bytes);
                    }
                    Some(Command::Resize(s)) => {
                        *size = s;
                        if let Some(master) = &self.master
                            && let Err(e) = master.resize(native_size(s))
                        {
                            log::debug!("session {}: cannot resize the terminal: {e}", ctx.id);
                        }
                    }
                    Some(Command::Reconnect) => break Some(Outcome::Restart),
                    Some(Command::Disconnect) => break Some(Outcome::Ended(i18n::process_stopped())),
                    Some(Command::Close) | None => break Some(Outcome::Closed),
                    Some(Command::AddForward(_, reply)) => {
                        let _ = reply.send(Err(Error::invalid(i18n::local_no_forwarding())));
                    }
                    Some(Command::RemoveForward(_)) => {}
                },
            }
        };
        if !out.is_empty() {
            ctx.sink.output(out);
        }
        let exited = exit.is_some();
        let outcome = requested.unwrap_or_else(|| {
            Outcome::Ended(match exit.flatten() {
                Some(status) => i18n::process_exited(&describe(&status)),
                None => i18n::process_ended(),
            })
        });
        self.stop(exited);
        outcome
    }

    /// Ends the program unless it has exited, and closes the terminal.
    fn stop(mut self, exited: bool) {
        // Once it has exited (and was waited for), its process id may belong to another one.
        if !exited && self.exited.try_recv().is_err() {
            let _ = self.killer.kill();
            #[cfg(unix)]
            if let Some(pid) = self.pid.and_then(|p| libc::pid_t::try_from(p).ok()) {
                // Shells exit on SIGHUP; a program ignoring it is killed after a moment. The
                // exit is awaited first, so a process id that was freed is never signalled.
                let exited = std::mem::replace(&mut self.exited, oneshot::channel().1);
                tokio::spawn(async move {
                    if tokio::time::timeout(KILL_GRACE, exited).await.is_err() {
                        // SAFETY: kill(2) only sends a signal; `pid` is our child, which has
                        // not exited (it would have been reported).
                        unsafe { libc::kill(pid, libc::SIGKILL) };
                    }
                });
            }
        }
        self.release();
        // Dropping the input channel ends the writer thread.
    }

    /// Closes the terminal on a blocking thread: closing ConPTY waits until its output has
    /// been read, which the reader thread keeps doing.
    fn release(&mut self) {
        if let Some(master) = self.master.take() {
            tokio::task::spawn_blocking(move || drop(master));
        }
    }
}

fn describe(status: &ExitStatus) -> String {
    match status.signal() {
        Some(signal) => i18n::exit_signal(signal),
        // Windows reports crashes and Ctrl+C as NTSTATUS values, readable in hex.
        None if status.exit_code() >= 0x8000_0000 => {
            i18n::exit_code(format!("0x{:08X}", status.exit_code()))
        }
        None => i18n::exit_code(status.exit_code()),
    }
}

fn native_size(size: PtySize) -> portable_pty::PtySize {
    let clamp = |v: u32| u16::try_from(v).unwrap_or(u16::MAX);
    portable_pty::PtySize {
        rows: clamp(size.rows),
        cols: clamp(size.cols),
        pixel_width: clamp(size.width_px),
        pixel_height: clamp(size.height_px),
    }
}

fn thread(name: &str, f: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .map(drop)
}

fn read_output(mut reader: Box<dyn Read + Send>, tx: mpsc::Sender<Vec<u8>>) {
    let mut buf = vec![0; 64 * 1024];
    let mut listening = true;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                // When the session stops listening, the output is still read to its end:
                // ConPTY cannot close while output is waiting.
                if listening && tx.blocking_send(buf[..n].to_vec()).is_err() {
                    listening = false;
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            // EIO: the program and everything it started have closed the terminal.
            Err(_) => break,
        }
    }
}

fn write_input(mut writer: Box<dyn Write + Send>, rx: std::sync::mpsc::Receiver<Vec<u8>>) {
    while let Ok(mut data) = rx.recv() {
        // What piled up meanwhile (a paste arrives in pieces) goes in one write.
        while let Ok(more) = rx.try_recv() {
            data.extend_from_slice(&more);
        }
        if writer
            .write_all(&data)
            .and_then(|()| writer.flush())
            .is_err()
        {
            break;
        }
    }
}

/// Input goes through a duplicate of the terminal's descriptor: portable-pty's own writer
/// sends a newline and end-of-file when dropped, which would run whatever is typed at the
/// prompt when the tab closes.
#[cfg(unix)]
fn writer(master: &dyn MasterPty) -> std::io::Result<Box<dyn Write + Send>> {
    use std::os::fd::BorrowedFd;
    let fd = master
        .as_raw_fd()
        .ok_or_else(|| std::io::Error::other("the terminal has no file descriptor"))?;
    // SAFETY: `fd` is the open descriptor of `master`, which outlives this call; the
    // duplicate is a descriptor of its own.
    let fd = unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()?;
    Ok(Box::new(std::fs::File::from(fd)))
}

#[cfg(windows)]
fn writer(master: &dyn MasterPty) -> std::io::Result<Box<dyn Write + Send>> {
    master.take_writer().map_err(std::io::Error::other)
}

/// ConPTY on a real Windows: CI runs the core's unit tests there. Unix is covered by
/// `tests/local.rs`.
#[cfg(all(test, windows))]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::mpsc;

    use crate::secrets::Secrets;
    use crate::{Core, EventSink, PtySize, SessionEvent, SessionStatus};

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

    #[tokio::test(flavor = "multi_thread")]
    async fn runs_command_prompt_in_conpty() {
        let dir = std::env::temp_dir().join(format!("nexssh-conpty-{}", std::process::id()));
        let core = Core::with_secrets(&dir, Secrets::in_memory()).unwrap();
        let cmd = crate::local::shells()
            .into_iter()
            .find(|s| s.id == "cmd")
            .expect("cmd.exe is found");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let id =
            core.sessions
                .open_local(cmd.command(None), PtySize::new(100, 30), Arc::new(Sink(tx)));

        let mut output = String::new();
        let mut answered = false;
        let mut typed = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        let (reason, failed) = loop {
            let ev = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out; output: {output:?}"))
                .expect("the session's events ended");
            match ev {
                Ev::Output(data) => {
                    output.push_str(&String::from_utf8_lossy(&data));
                    // ConPTY asks where the cursor is and waits for the terminal's answer.
                    if !answered && output.contains("\x1b[6n") {
                        core.sessions.write(id, b"\x1b[1;1R".to_vec()).unwrap();
                        answered = true;
                    }
                    // Typed once cmd has printed its banner.
                    if !typed && output.contains("Microsoft") {
                        let line = b"set /a 12345*3 & exit 3\r".to_vec();
                        core.sessions.write(id, line).unwrap();
                        typed = true;
                    }
                }
                Ev::Event(SessionEvent::Status {
                    status: SessionStatus::Disconnected,
                    message,
                    failed,
                }) => break (message.unwrap_or_default(), failed),
                Ev::Event(_) => {}
            }
        };
        // The last output arrives before the end is reported.
        assert!(output.contains("37035"), "{output:?}");
        // Language-neutral: another unit test may switch the language meanwhile.
        assert!(reason.contains('3') && !failed, "{reason}");
        core.sessions.close(id);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
