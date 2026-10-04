//! Commands run on a session's connection without a terminal, the way `ssh host command`
//! runs them: for AI agents (see the desktop's `agents`). Each command gets a channel of its
//! own on the connection of a connected session ([`crate::SessionManager::exec`]), so it
//! needs no second login and does not touch what the user types in the terminal.
//!
//! There is no pseudo-terminal: programs see pipes (no colours, no pagers, no prompts), and
//! standard input ends right after what the caller gives (like `ssh host command < file`), so
//! a program waiting for input gets end-of-file instead of hanging.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, Handle};
use russh::{ChannelMsg, ChannelWriteHalf, Sig};

use crate::error::{Error, Result};
use crate::i18n;
use crate::session::handler::ClientHandler;

/// How a command is run.
#[derive(Debug, Clone)]
pub struct ExecOptions {
    /// When the command is stopped (its channel closed, `SIGKILL` asked for).
    pub timeout: Duration,
    /// Bytes kept of each of standard output and standard error: the beginning and the end,
    /// what is in between is only counted.
    pub max_output: usize,
    /// Given to the command's standard input, which then ends.
    pub stdin: Vec<u8>,
}

impl Default for ExecOptions {
    fn default() -> Self {
        ExecOptions {
            timeout: Duration::from_secs(60),
            max_output: 64 * 1024,
            stdin: Vec::new(),
        }
    }
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    Code(u32),
    /// Killed by a signal (`TERM`, `KILL`, `SEGV`…).
    Signal(String),
}

#[derive(Debug)]
pub struct ExecOutput {
    pub stdout: Captured,
    pub stderr: Captured,
    /// `None` when the server closed the channel without saying.
    pub exit: Option<Exit>,
    /// The command did not finish in time and was stopped.
    pub timed_out: bool,
}

/// What is kept of one output stream: its beginning and, when there is more than fits, its
/// end (errors and summaries are usually there).
#[derive(Debug, Default)]
pub struct Captured {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    head_limit: usize,
    tail_limit: usize,
    total: u64,
}

impl Captured {
    pub fn new(limit: usize) -> Captured {
        let head_limit = limit / 4;
        Captured {
            head_limit,
            tail_limit: limit - head_limit,
            ..Captured::default()
        }
    }

    pub fn push(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        let room = self.head_limit.saturating_sub(self.head.len());
        if room > 0 {
            let n = room.min(data.len());
            self.head.extend_from_slice(&data[..n]);
            data = &data[n..];
        }
        if data.len() >= self.tail_limit {
            self.tail.clear();
            self.tail.extend(&data[data.len() - self.tail_limit..]);
        } else {
            let excess = (self.tail.len() + data.len()).saturating_sub(self.tail_limit);
            self.tail.drain(..excess);
            self.tail.extend(data);
        }
    }

    /// Bytes the stream produced in all.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Bytes between the beginning and the end that were not kept.
    pub fn omitted(&self) -> u64 {
        self.total - (self.head.len() + self.tail.len()) as u64
    }

    /// The beginning (everything, when nothing was left out) and the end.
    pub fn parts(&self) -> (&[u8], Vec<u8>) {
        (&self.head, self.tail.iter().copied().collect())
    }
}

pub(crate) async fn run(
    handle: &Handle<ClientHandler>,
    command: &str,
    options: ExecOptions,
) -> Result<ExecOutput> {
    let channel = handle.channel_open_session().await?;
    let (mut reader, writer) = channel.split();
    let writer = Arc::new(writer);
    writer.exec(true, command).await?;

    // The input waits until the server took the command.
    let mut stdin = Some(options.stdin);
    let mut input: Option<tokio::task::JoinHandle<()>> = None;
    let mut output = ExecOutput {
        stdout: Captured::new(options.max_output),
        stderr: Captured::new(options.max_output),
        exit: None,
        timed_out: false,
    };
    let deadline = tokio::time::sleep(options.timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            msg = reader.wait() => match msg {
                // The answer to the request is taken wherever it comes: other messages may
                // come first (OpenSSH widens a new channel's window right away).
                Some(ChannelMsg::Success) => {
                    if let Some(stdin) = stdin.take() {
                        input = Some(send_input(Arc::clone(&writer), stdin));
                    }
                }
                Some(ChannelMsg::Failure) if stdin.is_some() => {
                    return Err(Error::invalid(i18n::exec_refused()));
                }
                Some(ChannelMsg::Data { data }) => output.stdout.push(&data),
                Some(ChannelMsg::ExtendedData { data, ext: 1 }) => output.stderr.push(&data),
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    output.exit = Some(Exit::Code(exit_status));
                }
                Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                    output.exit = Some(Exit::Signal(signal(&signal_name)));
                }
                Some(ChannelMsg::Close) | None => {
                    let nothing = output.exit.is_none() && output.stdout.total() == 0;
                    if stdin.is_some() && nothing {
                        return Err(Error::Disconnected(i18n::connection_closed()));
                    }
                    break;
                }
                Some(_) => {}
            },
            () = &mut deadline => {
                output.timed_out = true;
                // Servers may not act on signals; closing the channel is what surely ends
                // it on this side.
                let _ = writer.signal(Sig::KILL).await;
                let _ = writer.close().await;
                break;
            }
        }
    }
    if let Some(input) = input {
        input.abort();
    }
    Ok(output)
}

/// Writes the input and ends it, in a task of its own: a command may write more than the
/// channel's window before it reads, and its output must keep being read meanwhile.
fn send_input(
    writer: Arc<ChannelWriteHalf<client::Msg>>,
    stdin: Vec<u8>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if !stdin.is_empty() && writer.data_bytes(stdin).await.is_err() {
            return;
        }
        let _ = writer.eof().await;
    })
}

fn signal(sig: &Sig) -> String {
    match sig {
        Sig::Custom(name) => name.clone(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(c: &Captured) -> (String, String) {
        let (head, tail) = c.parts();
        (
            String::from_utf8_lossy(head).into_owned(),
            String::from_utf8_lossy(&tail).into_owned(),
        )
    }

    #[test]
    fn keeps_everything_that_fits() {
        let mut c = Captured::new(16);
        c.push(b"hello ");
        c.push(b"world");
        assert_eq!(c.total(), 11);
        assert_eq!(c.omitted(), 0);
        let (head, tail) = text(&c);
        assert_eq!(format!("{head}{tail}"), "hello world");
    }

    #[test]
    fn keeps_the_beginning_and_the_end() {
        let mut c = Captured::new(16);
        for i in 0..100u8 {
            c.push(&[b'a' + i % 26]);
        }
        assert_eq!(c.total(), 100);
        assert_eq!(c.omitted(), 84);
        let (head, tail) = text(&c);
        assert_eq!(head, "abcd");
        // The last twelve of 100 letters cycling through the alphabet: positions 88..100.
        assert_eq!(tail, "klmnopqrstuv");

        // One big piece behaves the same as many small ones.
        let mut big = Captured::new(16);
        let data: Vec<u8> = (0..100u8).map(|i| b'a' + i % 26).collect();
        big.push(&data);
        assert_eq!(text(&big), (head, tail));
        assert_eq!(big.omitted(), 84);
    }
}
