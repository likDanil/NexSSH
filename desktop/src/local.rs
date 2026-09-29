//! IPC commands of local terminals, and folders to open them in that come from the command
//! line: `NexSSH --cwd <folder>` (what "Open with NexSSH" in Explorer runs).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use nexssh_core::i18n;
use nexssh_core::local::{self, ShellProfile};
use nexssh_core::{LocalCommand, PtySize, SessionId};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{Emitter, Manager, State};

use crate::commands::{AppState, CmdError, CmdResult, blocking};
use crate::sink::ChannelSink;

/// The event telling the page that [`launch_take`] has folders for it.
pub const LAUNCH_EVENT: &str = "launch";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalShells {
    shells: Vec<ShellProfile>,
    /// Windows' build number: xterm.js adapts to ConPTY by it.
    windows_build: Option<u32>,
}

#[tauri::command]
pub async fn local_shells() -> CmdResult<LocalShells> {
    blocking(|| LocalShells {
        shells: local::shells(),
        windows_build: local::windows_build(),
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalTarget {
    /// An id from [`local_shells`]; the default shell when `None`.
    profile: Option<String>,
    /// A command line to run instead (the custom shell of the settings).
    command: Option<String>,
    /// The folder to start in; the home folder when `None`.
    cwd: Option<String>,
}

/// Opens a local terminal; it is a session like an SSH one (same write, resize, reconnect
/// and close commands).
#[tauri::command]
pub async fn local_open(
    state: State<'_, AppState>,
    target: LocalTarget,
    cols: u32,
    rows: u32,
    on_event: Channel<InvokeResponseBody>,
) -> CmdResult<SessionId> {
    let cwd = target
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(nexssh_core::expand_tilde);
    let command = match target.command.as_deref().filter(|c| !c.trim().is_empty()) {
        Some(line) => LocalCommand::parse(line, cwd.as_deref())?,
        None => {
            let shells = blocking(local::shells).await?;
            let profile = match &target.profile {
                Some(id) => shells.iter().find(|s| &s.id == id),
                None => shells.iter().find(|s| s.default),
            };
            match profile {
                Some(profile) => profile.command(cwd.as_deref()),
                None => {
                    let id = target.profile.as_deref().unwrap_or_default();
                    return Err(CmdError::from(i18n::shell_not_installed(id)));
                }
            }
        }
    };
    let sink = Arc::new(ChannelSink::new(on_event));
    Ok(state
        .core
        .sessions
        .open_local(command, PtySize::new(cols, rows), sink))
}

/// The system file dialog for the program of a custom shell; `None` when cancelled.
#[tauri::command]
pub async fn pick_program(
    window: tauri::WebviewWindow,
    title: String,
) -> CmdResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;

    let mut dialog = window.dialog().file().set_parent(&window).set_title(title);
    if cfg!(windows) {
        dialog = dialog.add_filter(i18n::programs_filter(), &["exe", "cmd", "bat"]);
    }
    // The dialog runs on the main thread; wait for its answer here.
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    dialog.pick_file(move |picked| {
        let _ = tx.try_send(picked);
    });
    let Some(picked) = rx.recv().await.flatten() else {
        return Ok(None);
    };
    let path = picked
        .into_path()
        .map_err(|e| CmdError::from(e.to_string()))?;
    Ok(Some(path.display().to_string()))
}

/// Shows or hides "Open with NexSSH" in the context menu of folders in Windows Explorer
/// (in the interface's language); nothing to do elsewhere.
#[tauri::command]
pub async fn explorer_menu(enabled: bool) -> CmdResult<()> {
    #[cfg(windows)]
    return Ok(blocking(move || crate::explorer::set(enabled)).await??);
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Ok(())
    }
}

// ---- folders from the command line --------------------------------------------------------

/// Folders asked for on the command line, of this start or of a later one handed over by
/// the single-instance plugin, until the page takes them.
#[derive(Default)]
pub struct Launches(Mutex<Vec<PathBuf>>);

impl Launches {
    /// Queues the folders asked for by `args` (relative ones are in `cwd`); whether there
    /// were any.
    pub fn add(&self, args: &[String], cwd: &Path) -> bool {
        let folders = requested_folders(args, cwd);
        let any = !folders.is_empty();
        if any {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend(folders);
        }
        any
    }
}

/// Queues the folders of a second start and tells the page.
pub fn handle_second_start(app: &tauri::AppHandle, args: &[String], cwd: &str) {
    if let Some(launches) = app.try_state::<Launches>()
        && launches.add(args, Path::new(cwd))
    {
        let _ = app.emit(LAUNCH_EVENT, ());
    }
}

/// Folders to open local terminals in, once: the page asks at start-up and on [`LAUNCH_EVENT`].
#[tauri::command]
pub fn launch_take(launches: State<'_, Launches>) -> Vec<String> {
    std::mem::take(&mut *launches.0.lock().unwrap_or_else(|e| e.into_inner()))
        .into_iter()
        .map(|p| p.display().to_string())
        .collect()
}

/// `--cwd <folder>` and `--cwd=<folder>` in `args` (the program comes first), relative
/// ones taken in `cwd`.
fn requested_folders(args: &[String], cwd: &Path) -> Vec<PathBuf> {
    let mut folders = Vec::new();
    let mut args = args.iter().skip(1);
    while let Some(arg) = args.next() {
        let value = match arg.strip_prefix("--cwd") {
            Some("") => args.next().map(String::as_str),
            Some(rest) => rest.strip_prefix('='),
            None => None,
        };
        if let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) {
            folders.push(folder(value, cwd));
        }
    }
    folders
}

fn folder(value: &str, cwd: &Path) -> PathBuf {
    let mut value = value.to_string();
    // Explorer gives a drive's root as `C:\`, so `"%V"` becomes `"C:\"`, where the
    // backslash escapes the closing quote: the argument arrives as `C:"`. A quote cannot
    // be part of a Windows path.
    if cfg!(windows) && value.ends_with('"') {
        value.pop();
        value.push('\\');
    }
    let path = nexssh_core::expand_tilde(&value);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("NexSSH")
            .chain(list.iter().copied())
            .map(String::from)
            .collect()
    }

    #[test]
    fn reads_folders_from_the_command_line() {
        let cwd = Path::new("/home/me");
        assert!(requested_folders(&args(&[]), cwd).is_empty());
        assert_eq!(
            requested_folders(&args(&["--cwd", "/srv/app", "--cwd=src", "--other"]), cwd),
            [PathBuf::from("/srv/app"), PathBuf::from("/home/me/src")]
        );
        // A value is required; the program's own name is never taken as an argument.
        assert!(requested_folders(&args(&["--cwd"]), cwd).is_empty());
        assert!(requested_folders(&args(&["--cwd", "  "]), cwd).is_empty());
        assert!(requested_folders(&["--cwd".into()], cwd).is_empty());
        assert!(requested_folders(&args(&["--cwdx", "/srv"]), cwd).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_root_survives_explorers_quoting() {
        let cwd = Path::new(r"C:\Users\me");
        assert_eq!(
            requested_folders(&args(&["--cwd", r#"D:""#]), cwd),
            [PathBuf::from(r"D:\")]
        );
        assert_eq!(
            requested_folders(&args(&["--cwd", r"C:\Users\me\My Projects"]), cwd),
            [PathBuf::from(r"C:\Users\me\My Projects")]
        );
    }

    #[test]
    fn launches_are_taken_once() {
        let launches = Launches::default();
        assert!(!launches.add(&args(&[]), Path::new("/")));
        assert!(launches.add(&args(&["--cwd", "/a"]), Path::new("/")));
        assert!(launches.add(&args(&["--cwd=/b"]), Path::new("/")));
        let taken = std::mem::take(&mut *launches.0.lock().unwrap());
        assert_eq!(taken, [PathBuf::from("/a"), PathBuf::from("/b")]);
    }
}
