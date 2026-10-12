//! Files and folders of the files drawer dragged out of the window into Explorer, onto the
//! desktop or into a folder (Windows): `nexssh-dragout` offers them as virtual files, and
//! Explorer copies them as its own, reading each from a stream that this fills from the server
//! as a download would. Explorer copies them one at a time, so the next few are read ahead: a
//! small file is mostly round trips.
//!
//! Development builds drop every drag into the folder `NEXSSH_DRAG_INTO` names, if it is set,
//! without the mouse (`nexssh_dragout::drop_into`): the whole way from the drawer to the copy
//! can be tried from a script.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use nexssh_core::sftp::{Planned, Sftp};
use nexssh_core::{Error, i18n};
use nexssh_dragout::{Ended, Item, MAX_PATH_LEN, Outcome, Source};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncReadExt, DuplexStream};
use tokio::runtime::Handle;
use tokio::sync::oneshot;
use windows::Win32::Foundation::HWND;

use crate::sftp::{DragProgress, Transfers, lock};

/// Files read ahead of the one Explorer copies.
const READ_AHEAD: usize = 8;
/// What a file's stream holds before reading from the server waits for Explorer.
const PIPE: usize = 1024 * 1024;
/// Progress is passed on at most this often.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// A drag of remote files and folders.
pub struct Drag {
    pub sftp: Arc<Sftp>,
    pub paths: Vec<String>,
    pub cancel: Arc<AtomicBool>,
    pub progress: Channel<DragProgress>,
    pub app: AppHandle,
    pub transfer_id: u64,
}

/// Drags `drag`'s files out of `window`, on its thread, while the mouse button that started
/// the drag is down; whether they were dropped where files are taken.
pub async fn run(window: &tauri::WebviewWindow, drag: Drag) -> Result<bool, String> {
    let runtime = Handle::current();
    // Listed from the start, so that a folder's list is ready (or nearly) when Explorer asks.
    let (planned, plan) = oneshot::channel();
    let (sftp, paths, cancel) = (Arc::clone(&drag.sftp), drag.paths, Arc::clone(&drag.cancel));
    tauri::async_runtime::spawn(async move {
        let _ = planned.send(sftp.plan(&paths, &cancel).await);
    });
    let source = Arc::new(SftpSource {
        runtime,
        sftp: drag.sftp,
        plan: Mutex::new(Some(plan)),
        planned: OnceLock::new(),
        too_long: AtomicU64::new(0),
        cancel: drag.cancel,
        pipes: Mutex::new(Pipes::default()),
        progress: Arc::new(Progress::new(drag.progress)),
        app: drag.app,
        transfer_id: drag.transfer_id,
    });

    let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as isize;
    let (ended, outcome) = oneshot::channel();
    let dragged: Arc<dyn Source> = source;
    #[cfg(debug_assertions)]
    let into = std::env::var_os("NEXSSH_DRAG_INTO").map(std::path::PathBuf::from);
    window
        .run_on_main_thread(move || {
            #[cfg(debug_assertions)]
            if let Some(folder) = into {
                let _ = ended
                    .send(nexssh_dragout::drop_into(&folder, dragged).map_err(|e| e.message()));
                return;
            }
            let outcome = nexssh_dragout::drag(HWND(hwnd as _), dragged).map_err(|e| e.message());
            let _ = ended.send(outcome);
        })
        .map_err(|e| e.to_string())?;
    let outcome = outcome.await.map_err(|_| i18n::cancelled())??;
    Ok(outcome == Outcome::Dropped)
}

struct SftpSource {
    runtime: Handle,
    sftp: Arc<Sftp>,
    /// The listing started with the drag, until Explorer asks for it.
    plan: Mutex<Option<oneshot::Receiver<nexssh_core::Result<Vec<Planned>>>>>,
    /// What it listed, by the indexes Explorer asks for files with.
    planned: OnceLock<Vec<Planned>>,
    /// Items left out: paths too long for Explorer.
    too_long: AtomicU64,
    cancel: Arc<AtomicBool>,
    pipes: Mutex<Pipes>,
    progress: Arc<Progress>,
    app: AppHandle,
    transfer_id: u64,
}

/// The files read ahead.
#[derive(Default)]
struct Pipes {
    /// By index, until Explorer opens them.
    ahead: BTreeMap<usize, Pipe>,
    /// The index to look at next for one to read ahead.
    next: usize,
}

/// A file being read from the server into a stream.
struct Pipe {
    reader: DuplexStream,
    /// How reading it from the server ended.
    result: oneshot::Receiver<nexssh_core::Result<()>>,
}

impl SftpSource {
    fn start(&self, item: &Planned) -> Pipe {
        let (mut writer, reader) = tokio::io::duplex(PIPE);
        let (ended, result) = oneshot::channel();
        let (sftp, cancel) = (Arc::clone(&self.sftp), Arc::clone(&self.cancel));
        let (remote, size) = (item.remote.clone(), item.size);
        self.runtime.spawn(async move {
            let read = sftp.read_into(&remote, size, &mut writer, &cancel).await;
            // The end of the stream, then how it ended.
            drop(writer);
            let _ = ended.send(read);
        });
        Pipe { reader, result }
    }
}

impl Source for SftpSource {
    fn items(&self) -> Result<Vec<Item>, String> {
        let plan = lock(&self.plan).take().ok_or_else(i18n::cancelled)?;
        let planned = self
            .runtime
            .block_on(plan)
            .map_err(|_| i18n::cancelled())?
            .map_err(|e| e.to_string())?;
        let items: Vec<Item> = planned
            .iter()
            .map(|p| Item {
                path: p.names.join("\\"),
                folder: p.folder,
                size: p.size,
                modified: p.modified,
            })
            .collect();
        let too_long = items
            .iter()
            .filter(|i| i.path.encode_utf16().count() > MAX_PATH_LEN)
            .count();
        self.too_long.store(too_long as u64, Ordering::Relaxed);
        self.progress.total(planned.iter().map(|p| p.size).sum());
        let _ = self.planned.set(planned);
        Ok(items)
    }

    fn open(&self, index: usize) -> std::io::Result<Box<dyn Read + Send>> {
        let planned = self.planned.get().map(Vec::as_slice).unwrap_or_default();
        let item = planned
            .get(index)
            .filter(|p| !p.folder)
            .ok_or_else(|| std::io::Error::from(ErrorKind::NotFound))?;
        let pipe = {
            let mut pipes = lock(&self.pipes);
            let pipe = pipes
                .ahead
                .remove(&index)
                .unwrap_or_else(|| self.start(item));
            pipes.next = pipes.next.max(index + 1);
            while pipes.ahead.len() < READ_AHEAD && pipes.next < planned.len() {
                let next = pipes.next;
                pipes.next += 1;
                if !planned[next].folder {
                    let ahead = self.start(&planned[next]);
                    pipes.ahead.insert(next, ahead);
                }
            }
            pipe
        };
        Ok(Box::new(PipeReader {
            pipe,
            runtime: self.runtime.clone(),
            progress: Arc::clone(&self.progress),
            ended: false,
        }))
    }

    fn finished(&self, ended: Ended) {
        // Files read ahead and not taken (skipped, or the copy stopped) stop being read, and
        // so does the listing if the drop came before it ended.
        lock(&self.pipes).ahead.clear();
        self.cancel.store(true, Ordering::Relaxed);
        self.app.state::<Transfers>().ended(self.transfer_id);
        let too_long = self.too_long.load(Ordering::Relaxed);
        let (state, error) = match ended {
            Ended::Cancelled => ("cancelled", None),
            Ended::Failed(error) => ("failed", Some(error)),
            Ended::Done if too_long > 0 => {
                ("failed", Some(i18n::drag_paths_too_long(too_long as usize)))
            }
            Ended::Done => ("done", None),
        };
        self.progress.end(state, error);
    }
}

/// A file's content as Explorer reads it: what reading it from the server puts in its stream.
struct PipeReader {
    pipe: Pipe,
    runtime: Handle,
    progress: Arc<Progress>,
    ended: bool,
}

impl Read for PipeReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() || self.ended {
            return Ok(0);
        }
        // On COM's threads, outside the runtime: waiting here holds no task up.
        let got = self.runtime.block_on(self.pipe.reader.read(buffer))?;
        if got > 0 {
            self.progress.moved(got as u64);
            return Ok(got);
        }
        // The end of the stream: the file is whole if reading it went well.
        self.ended = true;
        match self.runtime.block_on(&mut self.pipe.result) {
            Ok(Ok(())) => Ok(0),
            Ok(Err(Error::Cancelled)) | Err(_) => Err(std::io::Error::new(
                ErrorKind::ConnectionAborted,
                i18n::cancelled(),
            )),
            Ok(Err(e)) => Err(std::io::Error::other(e.to_string())),
        }
    }
}

/// The copy's progress, passed on to the page.
struct Progress {
    channel: Channel<DragProgress>,
    done: AtomicU64,
    total: AtomicU64,
    last: Mutex<Option<Instant>>,
}

impl Progress {
    fn new(channel: Channel<DragProgress>) -> Progress {
        Progress {
            channel,
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
            last: Mutex::new(None),
        }
    }

    fn send(&self, state: &'static str, error: Option<String>) {
        let _ = self.channel.send(DragProgress {
            done: self.done.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            state,
            error,
        });
    }

    fn total(&self, total: u64) {
        self.total.store(total, Ordering::Relaxed);
        self.send("running", None);
    }

    fn moved(&self, bytes: u64) {
        self.done.fetch_add(bytes, Ordering::Relaxed);
        let mut last = lock(&self.last);
        if last.is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL) {
            *last = Some(Instant::now());
            drop(last);
            self.send("running", None);
        }
    }

    fn end(&self, state: &'static str, error: Option<String>) {
        self.send(state, error);
    }
}
