//! The session task: connect, run the PTY shell, react to commands, reconnect on demand.

use std::sync::Arc;
use std::time::Duration;

use russh::{ChannelMsg, Pty};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::connect::{self, Connection};
use super::{Command, LogLevel, SessionCtx, SessionStatus, lock};
use crate::error::Error;
use crate::forward::Forwards;
use crate::model::{PtySize, Server};

/// Output is coalesced to keep IPC traffic low under heavy output; a lone chunk
/// (e.g. a keystroke echo) is still delivered immediately.
const FLUSH_BYTES: usize = 64 * 1024;
const FLUSH_INTERVAL: Duration = Duration::from_millis(8);

enum Outcome {
    Closed,
    Reconnect,
    Lost(String),
}

enum Control {
    Close,
    Reconnect,
    /// The user aborted a connection attempt.
    Stop,
}

enum Input {
    Data(Vec<u8>),
    Resize(PtySize),
}

pub(crate) async fn run(
    ctx: Arc<SessionCtx>,
    server: Server,
    mut size: PtySize,
    mut rx: mpsc::UnboundedReceiver<Command>,
) {
    loop {
        ctx.status(SessionStatus::Connecting, None);
        let attempt = tokio::select! {
            result = connect::establish(&ctx, &server) => Some(result),
            control = idle(&mut rx, &mut size, true) => match control {
                Control::Close => break,
                Control::Reconnect => continue,
                Control::Stop => None,
            },
        };
        let outcome = match attempt {
            None => Outcome::Lost("Disconnected".into()),
            Some(Ok(conn)) => {
                if !server.id.is_empty() {
                    let store = Arc::clone(&ctx.shared.store);
                    let id = server.id.clone();
                    let _ = tokio::task::spawn_blocking(move || store.touch(&id)).await;
                }
                interactive(&ctx, conn, &server, &mut size, &mut rx).await
            }
            Some(Err(Error::Cancelled)) => Outcome::Lost("Authentication cancelled".into()),
            Some(Err(e)) => {
                ctx.log(LogLevel::Error, e.to_string());
                Outcome::Lost(e.to_string())
            }
        };
        match outcome {
            Outcome::Closed => break,
            Outcome::Reconnect => continue,
            Outcome::Lost(reason) => {
                ctx.status(SessionStatus::Disconnected, Some(reason));
                match idle(&mut rx, &mut size, false).await {
                    Control::Close => break,
                    Control::Reconnect | Control::Stop => continue,
                }
            }
        }
    }
    lock(&ctx.shared.sessions).remove(&ctx.id);
    ctx.status(SessionStatus::Closed, None);
}

/// Handles commands while there is no live connection (`connecting`: an attempt is
/// in progress and may be aborted).
async fn idle(
    rx: &mut mpsc::UnboundedReceiver<Command>,
    size: &mut PtySize,
    connecting: bool,
) -> Control {
    loop {
        match rx.recv().await {
            None | Some(Command::Close) => return Control::Close,
            Some(Command::Reconnect) => return Control::Reconnect,
            Some(Command::Disconnect) if connecting => return Control::Stop,
            Some(Command::Disconnect) => {}
            Some(Command::Resize(s)) => *size = s,
            Some(Command::Write(_)) | Some(Command::RemoveForward(_)) => {}
            Some(Command::AddForward(_, reply)) => {
                let _ = reply.send(Err(Error::Disconnected("not connected".into())));
            }
        }
    }
}

/// Terminal modes sent with the PTY request (mostly the OpenSSH defaults).
fn terminal_modes() -> Vec<(Pty, u32)> {
    vec![
        (Pty::VINTR, 3),
        (Pty::VERASE, 127),
        (Pty::ECHO, 1),
        (Pty::ICRNL, 1),
        (Pty::IUTF8, 1),
        (Pty::TTY_OP_ISPEED, 38400),
        (Pty::TTY_OP_OSPEED, 38400),
    ]
}

async fn interactive(
    ctx: &Arc<SessionCtx>,
    conn: Connection,
    server: &Server,
    size: &mut PtySize,
    rx: &mut mpsc::UnboundedReceiver<Command>,
) -> Outcome {
    let channel = async {
        let channel = conn.handle.channel_open_session().await?;
        channel
            .request_pty(
                false,
                "xterm-256color",
                size.cols,
                size.rows,
                size.width_px,
                size.height_px,
                &terminal_modes(),
            )
            .await?;
        channel.request_shell(false).await?;
        Ok::<_, Error>(channel)
    }
    .await;
    let channel = match channel {
        Ok(c) => c,
        Err(e) => {
            conn.disconnect().await;
            return Outcome::Lost(format!("could not start a shell: {e}"));
        }
    };
    let mut forwards = Forwards::new(
        Arc::clone(ctx),
        Arc::clone(&conn.handle),
        Arc::clone(&conn.state),
    );
    for spec in &server.forwards {
        if let Err(e) = forwards.start(spec.clone(), true).await {
            ctx.log(
                LogLevel::Warn,
                format!("Port forwarding {}: {e}", spec.describe()),
            );
        }
    }
    forwards.publish();
    // Announced after saved forwards are listening, so "connected" means fully ready.
    ctx.status(SessionStatus::Connected, None);

    // Writes go through their own task so a full remote window (e.g. a huge paste)
    // never stops us from reading output, which could otherwise deadlock.
    let (mut reader, writer) = channel.split();
    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Input>();
    let writer_task = tokio::spawn(async move {
        while let Some(input) = input_rx.recv().await {
            let ok = match input {
                Input::Data(bytes) => writer.data_bytes(bytes).await.is_ok(),
                Input::Resize(s) => writer
                    .window_change(s.cols, s.rows, s.width_px, s.height_px)
                    .await
                    .is_ok(),
            };
            if !ok {
                break;
            }
        }
    });

    let mut out: Vec<u8> = Vec::new();
    let mut last_flush = Instant::now() - FLUSH_INTERVAL;
    let flush_timer = tokio::time::sleep(Duration::from_secs(3600));
    tokio::pin!(flush_timer);
    let mut armed = false;
    let mut exit: Option<String> = None;

    let requested = loop {
        tokio::select! {
            msg = reader.wait() => match msg {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
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
                Some(ChannelMsg::ExitStatus { exit_status }) => exit = Some(format!("exit code {exit_status}")),
                Some(ChannelMsg::ExitSignal { signal_name, .. }) => exit = Some(format!("signal {signal_name:?}")),
                Some(ChannelMsg::Close) | None => break None,
                Some(_) => {}
            },
            () = &mut flush_timer, if armed => {
                if !out.is_empty() {
                    ctx.sink.output(std::mem::take(&mut out));
                }
                last_flush = Instant::now();
                armed = false;
            }
            cmd = rx.recv() => match cmd {
                Some(Command::Write(bytes)) => {
                    let _ = input_tx.send(Input::Data(bytes));
                }
                Some(Command::Resize(s)) => {
                    *size = s;
                    let _ = input_tx.send(Input::Resize(s));
                }
                Some(Command::Reconnect) => break Some(Outcome::Reconnect),
                Some(Command::Disconnect) => break Some(Outcome::Lost("Disconnected".into())),
                Some(Command::Close) | None => break Some(Outcome::Closed),
                Some(Command::AddForward(spec, reply)) => {
                    let result = forwards.start(spec, false).await;
                    forwards.publish();
                    let _ = reply.send(result);
                }
                Some(Command::RemoveForward(id)) => {
                    forwards.stop(id).await;
                    forwards.publish();
                }
            }
        }
    };
    if !out.is_empty() {
        ctx.sink.output(out);
    }
    writer_task.abort();
    forwards.stop_all().await;
    forwards.publish();

    let outcome = match requested {
        Some(outcome) => outcome,
        None => {
            let reason = match exit {
                Some(exit) => format!("Session ended ({exit})"),
                None => match lost_reason(&conn).await {
                    Some(reason) => format!("Connection lost: {reason}"),
                    None => "Connection closed".into(),
                },
            };
            Outcome::Lost(reason)
        }
    };
    conn.disconnect().await;
    outcome
}

/// The disconnect reason is recorded by the handler as the connection shuts down.
async fn lost_reason(conn: &Connection) -> Option<String> {
    for _ in 0..6 {
        if let Some(reason) = conn.state.reason() {
            return Some(reason);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}
