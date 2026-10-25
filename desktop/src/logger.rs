//! A tiny logger (avoids pulling in a logging framework): lines go to stderr and to
//! `logs/nexssh.log` in the data folder, so release builds, which have no console, keep them
//! too. Settings → About saves them into one file to send along with a report of a problem.
//! Level from `NEXSSH_LOG` (error/warn/info/debug/trace); default: info for NexSSH's own lines
//! (and the page's), warn for the libraries'.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use log::{Level, LevelFilter, Log, Metadata, Record};
use nexssh_core::i18n;
use serde::Serialize;

use crate::commands::{CmdError, CmdResult, blocking};

const FILE: &str = "nexssh.log";
/// The lines before the last time the log grew past `MAX_SIZE`.
const OLD_FILE: &str = "nexssh.old.log";
const MAX_SIZE: u64 = 2 * 1024 * 1024;
/// Lines from the page (window errors), cut to this many characters.
const MAX_PAGE_LINE: usize = 4000;

static LOGGER: OnceLock<Logger> = OnceLock::new();

struct Logger {
    /// For NexSSH's own targets (`nexssh_*`) and the page's.
    own: LevelFilter,
    others: LevelFilter,
    /// `None` when the folder has no log (it could not be created or opened).
    file: Mutex<Option<LogFile>>,
}

struct LogFile {
    dir: PathBuf,
    file: Option<File>,
    size: u64,
}

impl LogFile {
    fn open(dir: PathBuf) -> std::io::Result<LogFile> {
        std::fs::create_dir_all(&dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(FILE))?;
        let size = file.metadata()?.len();
        let mut log = LogFile {
            dir,
            file: Some(file),
            size,
        };
        if log.size > MAX_SIZE {
            log.rotate();
        }
        Ok(log)
    }

    fn write(&mut self, line: &str) {
        if self.size + line.len() as u64 > MAX_SIZE {
            self.rotate();
        }
        if let Some(file) = &mut self.file
            && file.write_all(line.as_bytes()).is_ok()
        {
            self.size += line.len() as u64;
        }
    }

    /// The log so far becomes the old one (the one before it is dropped); a new one starts.
    fn rotate(&mut self) {
        self.file = None;
        let _ = std::fs::rename(self.dir.join(FILE), self.dir.join(OLD_FILE));
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(FILE))
            .ok();
        self.size = 0;
    }
}

fn own(target: &str) -> bool {
    target.starts_with("nexssh") || target == "page"
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let level = if own(metadata.target()) {
            self.own
        } else {
            self.others
        };
        metadata.level() <= level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        eprintln!(
            "[{:<5}] {}: {}",
            record.level(),
            record.target(),
            record.args()
        );
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(file) = file.as_mut() {
            file.write(&format!(
                "{} [{:<5}] {}: {}\n",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                record.level(),
                record.target(),
                record.args()
            ));
        }
    }

    fn flush(&self) {}
}

pub fn init() {
    let level = std::env::var("NEXSSH_LOG")
        .ok()
        .and_then(|v| v.parse::<LevelFilter>().ok());
    let file = nexssh_core::default_data_dir().and_then(|dir| LogFile::open(dir.join("logs")).ok());
    let logger = LOGGER.get_or_init(|| Logger {
        own: level.unwrap_or(LevelFilter::Info),
        others: level.unwrap_or(LevelFilter::Warn),
        file: Mutex::new(file),
    });
    if log::set_logger(logger).is_ok() {
        log::set_max_level(logger.own.max(logger.others));
        // A panic says where it happened in the log too, not only on a console nobody sees.
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            log::error!(
                "thread '{}' panicked: {info}",
                thread.name().unwrap_or("<unnamed>")
            );
            default(info);
        }));
    }
}

/// The folder of the log; `None` when there is none.
fn dir() -> Option<PathBuf> {
    let file = LOGGER.get()?.file.lock().unwrap_or_else(|e| e.into_inner());
    file.as_ref().map(|f| f.dir.clone())
}

/// The old log and the current one, as one text.
fn read(dir: &Path) -> std::io::Result<String> {
    let mut text = String::new();
    for name in [OLD_FILE, FILE] {
        match std::fs::read(dir.join(name)) {
            Ok(bytes) => text.push_str(&String::from_utf8_lossy(&bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(text)
}

fn no_log() -> CmdError {
    CmdError::from(i18n::logs_none())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exported {
    path: String,
}

/// Asks where to save the log and writes it there, under a line that says which NexSSH on
/// which system wrote it. `None` when the dialog was cancelled.
#[tauri::command]
pub async fn logs_export(
    window: tauri::WebviewWindow,
    title: String,
    file_name: String,
) -> CmdResult<Option<Exported>> {
    use tauri_plugin_dialog::DialogExt;
    let dir = dir().ok_or_else(no_log)?;
    let dialog = window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(title)
        .set_file_name(file_name)
        .add_filter(i18n::logs_filter(), &["log", "txt"]);
    // The dialog runs on the main thread; wait for its answer here.
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    dialog.save_file(move |picked| {
        let _ = tx.try_send(picked);
    });
    let Some(picked) = rx.recv().await.flatten() else {
        return Ok(None);
    };
    let path = picked
        .into_path()
        .map_err(|e| CmdError::from(e.to_string()))?;
    blocking(move || {
        let header = format!(
            "NexSSH {} · {} {} · {}\n\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S %:z"),
        );
        std::fs::write(&path, header + &read(&dir)?)?;
        log::info!("log saved to {}", path.display());
        Ok(Some(Exported {
            path: path.display().to_string(),
        }))
    })
    .await?
}

/// Shows the log in the system file manager.
#[tauri::command]
pub fn logs_reveal() -> CmdResult<()> {
    let dir = dir().ok_or_else(no_log)?;
    let file = dir.join(FILE);
    crate::sftp::reveal(if file.exists() { &file } else { &dir })
        .map_err(|e| CmdError::from(i18n::reveal_failed(e)))
}

/// A line from the page: errors in the window's code, which would be lost otherwise.
#[tauri::command]
pub fn log_page(level: String, message: String) {
    let level = match level.as_str() {
        "error" => Level::Error,
        "warn" => Level::Warn,
        _ => Level::Info,
    };
    let message: String = message.chars().take(MAX_PAGE_LINE).collect();
    log::log!(target: "page", level, "{message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_log_becomes_the_old_one() {
        let dir = std::env::temp_dir().join(format!("nexssh-logger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut log = LogFile::open(dir.clone()).unwrap();
        let line = format!("{}\n", "x".repeat(1023));
        for _ in 0..MAX_SIZE / 1024 {
            log.write(&line);
        }
        assert!(!dir.join(OLD_FILE).exists(), "not full yet");
        log.write("first of the new log\n");
        assert_eq!(
            std::fs::metadata(dir.join(OLD_FILE)).unwrap().len(),
            MAX_SIZE
        );
        let text = read(&dir).unwrap();
        assert_eq!(text.len() as u64, MAX_SIZE + 21);
        assert!(
            text.ends_with("x\nfirst of the new log\n"),
            "the old lines come first"
        );
        drop(log);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
