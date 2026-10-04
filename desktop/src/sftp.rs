//! SFTP commands for the files drawer. The logic lives in `nexssh_core::sftp`; this module
//! keeps transfers (cancel flags, uploads fed chunk by chunk from the UI, files and folders
//! picked or dropped for upload, which are read from disk here) and reveals downloaded files
//! in the system file manager.

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
    uploads: Mutex<HashMap<u64, Upload>>,
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

/// Creates the folders of an upload that do not exist yet (folder uploads merge into existing
/// ones), parents first, many at a time.
#[tauri::command]
pub async fn sftp_ensure_dirs(
    state: State<'_, AppState>,
    session_id: SessionId,
    paths: Vec<String>,
) -> CmdResult<()> {
    Ok(client(&state, session_id)
        .await?
        .ensure_dirs(&paths)
        .await?)
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

/// What the page posts with files dropped on it, followed by a number for the answer.
#[cfg(windows)]
const DROP_MESSAGE: &str = "nexssh-drop:";

/// Windows: files dropped on the page come to the app too. The page posts them with
/// `chrome.webview.postMessageWithAdditionalObjects("nexssh-drop:<id>", files)`, WebView2
/// tells their paths, and the page hears back as `files-picked` (with those it could tell):
/// uploaded with `sftp_upload_path`, they are read from disk here, much faster than pieces
/// sent from the page. A page cannot make such files up, so like the dialogs' these are
/// paths the user chose.
///
/// The message is a string because WebView2 stops at Tauri's own handler for anything else;
/// Tauri then reports in the console that it is not an IPC call, which does no harm.
#[cfg(windows)]
pub fn take_page_drops(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    let target = window.clone();
    window.with_webview(move |webview| {
        if let Err(e) = listen_for_drops(&webview.controller(), target) {
            log::warn!("files dropped on the page will be sent from it: {e}");
        }
    })
}

#[cfg(windows)]
fn listen_for_drops(
    controller: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
    window: tauri::WebviewWindow,
) -> windows::core::Result<()> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2File, ICoreWebView2WebMessageReceivedEventArgs,
        ICoreWebView2WebMessageReceivedEventArgs2,
    };
    use webview2_com::{CoTaskMemPWSTR, WebMessageReceivedEventHandler};
    use windows::core::{Interface, PWSTR};

    #[derive(Clone, Serialize)]
    struct PageDrop {
        id: u64,
        items: Vec<Picked>,
    }

    // The bindings mark every WebView2 call unsafe; these use objects WebView2 handed over.
    fn paths_of(
        args: &ICoreWebView2WebMessageReceivedEventArgs,
    ) -> windows::core::Result<Vec<PathBuf>> {
        let objects = unsafe {
            args.cast::<ICoreWebView2WebMessageReceivedEventArgs2>()?
                .AdditionalObjects()?
        };
        let mut count = 0;
        unsafe { objects.Count(&mut count)? };
        let mut paths = Vec::new();
        for index in 0..count {
            // What WebView2 does not give as a file (with a path) is left out; the page then
            // sends the drop itself.
            let Ok(file) = unsafe { objects.GetValueAtIndex(index)? }.cast::<ICoreWebView2File>()
            else {
                continue;
            };
            let mut path = PWSTR::null();
            unsafe { file.Path(&mut path)? };
            paths.push(PathBuf::from(CoTaskMemPWSTR::from(path).to_string()));
        }
        Ok(paths)
    }

    let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut text = PWSTR::null();
        unsafe { args.TryGetWebMessageAsString(&mut text)? };
        let text = CoTaskMemPWSTR::from(text).to_string();
        // Other messages (Tauri's own) are not for this handler.
        let Some(id) = text
            .strip_prefix(DROP_MESSAGE)
            .and_then(|id| id.parse::<u64>().ok())
        else {
            return Ok(());
        };
        let paths: Vec<PathBuf> = paths_of(&args)
            .unwrap_or_else(|e| {
                log::warn!("no paths for the files dropped on the page: {e}");
                Vec::new()
            })
            .into_iter()
            .filter(|p| p.exists())
            .collect();
        lock(&window.state::<Transfers>().picked).extend(paths.iter().cloned());
        let drop = PageDrop {
            id,
            items: paths.iter().map(|p| Picked::new(p)).collect(),
        };
        if let Err(e) = window.emit("files-picked", drop) {
            log::warn!("could not pass dropped files back to the page: {e}");
        }
        Ok(())
    }));
    let mut token = 0;
    unsafe {
        controller
            .CoreWebView2()?
            .add_WebMessageReceived(&handler, &mut token)
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
    let start = pick_start(&transfers);
    let Some(path) = crate::commands::choose_folder(&window, title, start).await? else {
        return Ok(None);
    };
    *lock(&transfers.last_pick) = path.parent().map(Path::to_path_buf);
    lock(&transfers.picked).insert(path.clone());
    Ok(Some(Picked::new(&path)))
}

/// Asks for local files to upload; empty when cancelled. Upload them with `sftp_upload_path`:
/// read from disk here, they go much faster than pieces sent from the page.
#[tauri::command]
pub async fn sftp_pick_files(
    window: tauri::WebviewWindow,
    transfers: State<'_, Transfers>,
    title: String,
) -> CmdResult<Vec<Picked>> {
    let start = pick_start(&transfers);
    let paths = crate::commands::choose_files(&window, title, start).await?;
    if let Some(dir) = paths.first().and_then(|p| p.parent()) {
        *lock(&transfers.last_pick) = Some(dir.to_path_buf());
    }
    lock(&transfers.picked).extend(paths.iter().cloned());
    Ok(paths.iter().map(|p| Picked::new(p)).collect())
}

/// Where the upload dialogs open: where the last pick was, else the home folder.
fn pick_start(transfers: &Transfers) -> PathBuf {
    lock(&transfers.last_pick)
        .clone()
        .filter(|d| d.is_dir())
        .or_else(nexssh_core::home_dir)
        .unwrap_or_else(sftp::download_dir)
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
    let upload = client(&state, session_id).await?.create(&path).await?;
    lock(&transfers.uploads).insert(transfer_id, upload);
    Ok(())
}

/// A piece of an upload: the raw request body, the transfer in the `x-transfer-id` header.
/// It returns once the piece is on its way (unless much is in flight already), so the page
/// can send the next one meanwhile.
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
    let mut upload = lock(&transfers.uploads)
        .remove(&id)
        .ok_or_else(|| CmdError::from(i18n::cancelled()))?;
    match upload.write(data).await {
        Ok(()) => {
            lock(&transfers.uploads).insert(id, upload);
            Ok(())
        }
        Err(e) => {
            upload.cancel().await;
            Err(e.into())
        }
    }
}

/// Waits for the upload's writes and closes the file (a failed upload is removed).
#[tauri::command]
pub async fn sftp_upload_end(transfers: State<'_, Transfers>, transfer_id: u64) -> CmdResult<()> {
    let upload = lock(&transfers.uploads)
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
    if let Some(upload) = upload {
        upload.cancel().await;
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
