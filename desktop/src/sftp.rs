//! SFTP commands for the files drawer. The logic lives in `nexssh_core::sftp`; this module
//! keeps transfers (cancel flags, uploads fed chunk by chunk from the UI, folders picked
//! for upload) and reveals downloaded files in the system file manager.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use nexssh_core::sftp::{self, Entry, Progress, Sftp, Upload};
use nexssh_core::{SessionId, i18n};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeBody, Request};
use tauri::{Emitter, Manager, State};

use crate::commands::{AppState, CmdError, CmdResult};

#[derive(Default)]
pub struct Transfers {
    cancel: Mutex<HashMap<u64, Arc<AtomicBool>>>,
    uploads: Mutex<HashMap<u64, (Arc<Sftp>, Upload)>>,
    /// Files this app downloaded: the only paths it will reveal.
    downloaded: Mutex<HashSet<PathBuf>>,
    /// Folders the user picked to download into: besides Downloads, the only places a
    /// download writes to, so the page cannot choose where files land.
    destinations: Mutex<HashSet<PathBuf>>,
    /// Files and folders the user picked or dragged in for upload: the only local paths an
    /// upload reads, so the page cannot make the app send other local files.
    picked: Mutex<HashSet<PathBuf>>,
    /// Where the dialogs were last used, to open them there again.
    last_destination: Mutex<Option<PathBuf>>,
    last_pick: Mutex<Option<PathBuf>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

async fn client(state: &AppState, session_id: SessionId) -> CmdResult<Arc<Sftp>> {
    Ok(state.core.sessions.sftp(session_id).await?)
}

/// Passes progress on at most every 100 ms, and always the end.
fn throttled(channel: Channel<Progress>) -> impl FnMut(Progress) + Send {
    let mut last_sent: Option<Instant> = None;
    move |p: Progress| {
        if p.done == p.total || last_sent.is_none_or(|t| t.elapsed() >= Duration::from_millis(100))
        {
            last_sent = Some(Instant::now());
            let _ = channel.send(p);
        }
    }
}

#[tauri::command]
pub async fn sftp_home(state: State<'_, AppState>, session_id: SessionId) -> CmdResult<String> {
    Ok(client(&state, session_id).await?.home().await?)
}

/// The absolute form of a path typed by the user (`~` is the home folder).
#[tauri::command]
pub async fn sftp_resolve(
    state: State<'_, AppState>,
    session_id: SessionId,
    path: String,
) -> CmdResult<String> {
    Ok(client(&state, session_id).await?.resolve(&path).await?)
}

#[tauri::command]
pub async fn sftp_list(
    state: State<'_, AppState>,
    session_id: SessionId,
    path: String,
) -> CmdResult<Vec<Entry>> {
    Ok(client(&state, session_id).await?.list(&path).await?)
}

#[tauri::command]
pub async fn sftp_mkdir(
    state: State<'_, AppState>,
    session_id: SessionId,
    path: String,
) -> CmdResult<()> {
    Ok(client(&state, session_id).await?.mkdir(&path).await?)
}

/// Creates a folder unless it exists already (folder uploads merge into existing ones).
#[tauri::command]
pub async fn sftp_ensure_dir(
    state: State<'_, AppState>,
    session_id: SessionId,
    path: String,
) -> CmdResult<()> {
    Ok(client(&state, session_id).await?.ensure_dir(&path).await?)
}

#[tauri::command]
pub async fn sftp_new_file(
    state: State<'_, AppState>,
    session_id: SessionId,
    path: String,
) -> CmdResult<()> {
    Ok(client(&state, session_id).await?.new_file(&path).await?)
}

#[tauri::command]
pub async fn sftp_chmod(
    state: State<'_, AppState>,
    session_id: SessionId,
    path: String,
    mode: u32,
    recursive: bool,
) -> CmdResult<()> {
    Ok(client(&state, session_id)
        .await?
        .chmod(&path, mode, recursive)
        .await?)
}

#[tauri::command]
pub async fn sftp_rename(
    state: State<'_, AppState>,
    session_id: SessionId,
    from: String,
    to: String,
) -> CmdResult<()> {
    Ok(client(&state, session_id).await?.rename(&from, &to).await?)
}

#[tauri::command]
pub async fn sftp_remove(
    state: State<'_, AppState>,
    session_id: SessionId,
    path: String,
) -> CmdResult<()> {
    Ok(client(&state, session_id).await?.remove(&path).await?)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Downloaded {
    path: String,
    name: String,
}

/// Downloads a file or folder into `dest` (a folder from `sftp_pick_destination`), or into
/// Downloads; `transfer_id` (chosen by the UI) lets `sftp_cancel` stop it.
#[tauri::command]
pub async fn sftp_download(
    state: State<'_, AppState>,
    transfers: State<'_, Transfers>,
    session_id: SessionId,
    path: String,
    dest: Option<String>,
    transfer_id: u64,
    on_progress: Channel<Progress>,
) -> CmdResult<Downloaded> {
    let dest = match dest.map(PathBuf::from) {
        None => sftp::download_dir(),
        Some(dir) if lock(&transfers.destinations).contains(&dir) => dir,
        Some(dir) => {
            let dir = dir.display().to_string();
            return Err(CmdError::from(i18n::sftp_no_such_file(&dir)));
        }
    };
    let sftp = client(&state, session_id).await?;
    let cancel = Arc::new(AtomicBool::new(false));
    lock(&transfers.cancel).insert(transfer_id, Arc::clone(&cancel));
    let result = sftp
        .download(&path, &dest, &mut throttled(on_progress), &cancel)
        .await;
    lock(&transfers.cancel).remove(&transfer_id);

    let local = result?;
    lock(&transfers.downloaded).insert(local.clone());
    Ok(Downloaded {
        name: local
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: local.display().to_string(),
    })
}

/// Asks for a local folder to download into; `None` when cancelled.
#[tauri::command]
pub async fn sftp_pick_destination(
    window: tauri::WebviewWindow,
    transfers: State<'_, Transfers>,
    title: String,
) -> CmdResult<Option<String>> {
    let start = lock(&transfers.last_destination)
        .clone()
        .filter(|d| d.is_dir())
        .unwrap_or_else(sftp::download_dir);
    let Some(dir) = crate::commands::choose_folder(&window, title, start).await? else {
        return Ok(None);
    };
    *lock(&transfers.last_destination) = Some(dir.clone());
    lock(&transfers.destinations).insert(dir.clone());
    Ok(Some(dir.display().to_string()))
}

/// A local file or folder the user chose to upload.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Picked {
    path: String,
    name: String,
    folder: bool,
}

impl Picked {
    fn new(path: &Path) -> Picked {
        Picked {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            path: path.display().to_string(),
            folder: path.is_dir(),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Dropped {
    items: Vec<Picked>,
    /// Where they were dropped, in page coordinates.
    x: f64,
    y: f64,
}

/// Files dragged into the window, from the native drag & drop handler (Linux: WebKitGTK
/// shows the page such a drag without the files). They may be uploaded like picked ones;
/// the page hears about them as `files-dragged` and uploads them if they are dropped on
/// the files drawer.
pub fn dragged(window: &tauri::Window, paths: &[PathBuf]) {
    let paths: Vec<&PathBuf> = paths.iter().filter(|p| p.exists()).collect();
    lock(&window.state::<Transfers>().picked).extend(paths.iter().map(|p| p.to_path_buf()));
    let items: Vec<Picked> = paths.into_iter().map(|p| Picked::new(p)).collect();
    if let Err(e) = window.emit("files-dragged", items) {
        log::warn!("could not pass dragged files to the page: {e}");
    }
}

/// Files dropped on the window, when the native handler gets the drop itself (the page
/// then gets no drop event): passed on as `files-dropped`.
pub fn dropped(window: &tauri::Window, paths: &[PathBuf], position: tauri::PhysicalPosition<f64>) {
    let paths: Vec<&PathBuf> = paths.iter().filter(|p| p.exists()).collect();
    lock(&window.state::<Transfers>().picked).extend(paths.iter().map(|p| p.to_path_buf()));
    // GTK reports widget coordinates, which are CSS pixels already (despite the type).
    let dropped = Dropped {
        items: paths.into_iter().map(|p| Picked::new(p)).collect(),
        x: position.x,
        y: position.y,
    };
    if let Err(e) = window.emit("files-dropped", dropped) {
        log::warn!("could not pass dropped files to the page: {e}");
    }
}

/// Asks for a local folder to upload (the page cannot pick folders the same way on every
/// system); `None` when cancelled. Upload it with `sftp_upload_path`.
#[tauri::command]
pub async fn sftp_pick_upload(
    window: tauri::WebviewWindow,
    transfers: State<'_, Transfers>,
    title: String,
) -> CmdResult<Option<Picked>> {
    let start = lock(&transfers.last_pick)
        .clone()
        .filter(|d| d.is_dir())
        .or_else(nexssh_core::home_dir)
        .unwrap_or_else(sftp::download_dir);
    let Some(path) = crate::commands::choose_folder(&window, title, start).await? else {
        return Ok(None);
    };
    *lock(&transfers.last_pick) = path.parent().map(Path::to_path_buf);
    lock(&transfers.picked).insert(path.clone());
    Ok(Some(Picked::new(&path)))
}

/// Uploads a folder picked with `sftp_pick_upload` (or a file or folder dropped on the
/// window) into `dir` and returns where it went; `transfer_id` (chosen by the UI) lets
/// `sftp_cancel` stop it.
#[tauri::command]
pub async fn sftp_upload_path(
    state: State<'_, AppState>,
    transfers: State<'_, Transfers>,
    session_id: SessionId,
    path: String,
    dir: String,
    transfer_id: u64,
    on_progress: Channel<Progress>,
) -> CmdResult<String> {
    let local = PathBuf::from(&path);
    if !lock(&transfers.picked).remove(&local) {
        return Err(CmdError::from(i18n::sftp_no_such_file(&path)));
    }
    let sftp = client(&state, session_id).await?;
    let cancel = Arc::new(AtomicBool::new(false));
    lock(&transfers.cancel).insert(transfer_id, Arc::clone(&cancel));
    let result = sftp
        .upload_path(&local, &dir, &mut throttled(on_progress), &cancel)
        .await;
    lock(&transfers.cancel).remove(&transfer_id);
    Ok(result?)
}

/// Starts an upload; the UI then sends the file with `sftp_upload_chunk`.
#[tauri::command]
pub async fn sftp_upload_begin(
    state: State<'_, AppState>,
    transfers: State<'_, Transfers>,
    session_id: SessionId,
    path: String,
    transfer_id: u64,
) -> CmdResult<()> {
    let sftp = client(&state, session_id).await?;
    let upload = sftp.create(&path).await?;
    lock(&transfers.uploads).insert(transfer_id, (sftp, upload));
    Ok(())
}

/// A piece of an upload: the raw request body, the transfer in the `x-transfer-id` header.
#[tauri::command]
pub async fn sftp_upload_chunk(
    request: Request<'_>,
    transfers: State<'_, Transfers>,
) -> CmdResult<()> {
    let id = request
        .headers()
        .get("x-transfer-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| CmdError::from(i18n::cancelled()))?;
    let InvokeBody::Raw(data) = request.body() else {
        return Err(CmdError::from("expected binary data"));
    };
    // Taken out while writing, so the lock is not held across the await.
    let (sftp, mut upload) = lock(&transfers.uploads)
        .remove(&id)
        .ok_or_else(|| CmdError::from(i18n::cancelled()))?;
    match upload.write(data).await {
        Ok(()) => {
            lock(&transfers.uploads).insert(id, (sftp, upload));
            Ok(())
        }
        Err(e) => {
            let path = upload.path().to_string();
            drop(upload);
            sftp.discard(&path).await;
            Err(e.into())
        }
    }
}

#[tauri::command]
pub async fn sftp_upload_end(transfers: State<'_, Transfers>, transfer_id: u64) -> CmdResult<()> {
    let (_, upload) = lock(&transfers.uploads)
        .remove(&transfer_id)
        .ok_or_else(|| CmdError::from(i18n::cancelled()))?;
    Ok(upload.finish().await?)
}

/// Stops a download, or abandons an upload and removes its partial file.
#[tauri::command]
pub async fn sftp_cancel(transfers: State<'_, Transfers>, transfer_id: u64) -> CmdResult<()> {
    if let Some(flag) = lock(&transfers.cancel).get(&transfer_id) {
        flag.store(true, Ordering::Relaxed);
    }
    let upload = lock(&transfers.uploads).remove(&transfer_id);
    if let Some((sftp, upload)) = upload {
        let path = upload.path().to_string();
        drop(upload);
        sftp.discard(&path).await;
    }
    Ok(())
}

/// Shows a downloaded file in the system file manager.
#[tauri::command]
pub fn reveal_download(transfers: State<'_, Transfers>, path: String) -> CmdResult<()> {
    let path = PathBuf::from(path);
    if !lock(&transfers.downloaded).contains(&path) || !path.exists() {
        return Err(CmdError::from(i18n::sftp_no_such_file(
            &path.display().to_string(),
        )));
    }
    reveal(&path).map_err(|e| CmdError::from(i18n::reveal_failed(e)))
}

#[cfg(windows)]
fn reveal(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    // explorer parses its own command line: `/select,"C:\path with spaces\file"`.
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn reveal(path: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()
        .map(|_| ())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn reveal(path: &std::path::Path) -> std::io::Result<()> {
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    std::process::Command::new("xdg-open")
        .arg(dir)
        .spawn()
        .map(|_| ())
}
