//! SFTP commands for the files drawer. The logic lives in `nexssh_core::sftp`; this module
//! keeps transfers (cancel flags, uploads fed chunk by chunk from the UI) and reveals
//! downloaded files in the system file manager.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use nexssh_core::sftp::{self, Entry, Progress, Sftp, Upload};
use nexssh_core::{SessionId, i18n};
use serde::Serialize;
use tauri::State;
use tauri::ipc::{Channel, InvokeBody, Request};

use crate::commands::{AppState, CmdError, CmdResult};

#[derive(Default)]
pub struct Transfers {
    cancel: Mutex<HashMap<u64, Arc<AtomicBool>>>,
    uploads: Mutex<HashMap<u64, (Arc<Sftp>, Upload)>>,
    /// Files this app downloaded: the only paths it will reveal.
    downloaded: Mutex<HashSet<PathBuf>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

async fn client(state: &AppState, session_id: SessionId) -> CmdResult<Arc<Sftp>> {
    Ok(state.core.sessions.sftp(session_id).await?)
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

/// Downloads a file or folder into the Downloads folder; `transfer_id` (chosen by the UI)
/// lets `sftp_cancel` stop it.
#[tauri::command]
pub async fn sftp_download(
    state: State<'_, AppState>,
    transfers: State<'_, Transfers>,
    session_id: SessionId,
    path: String,
    transfer_id: u64,
    on_progress: Channel<Progress>,
) -> CmdResult<Downloaded> {
    let sftp = client(&state, session_id).await?;
    let cancel = Arc::new(AtomicBool::new(false));
    lock(&transfers.cancel).insert(transfer_id, Arc::clone(&cancel));

    let mut last_sent = Instant::now() - Duration::from_secs(1);
    let mut report = |p: Progress| {
        if p.done == p.total || last_sent.elapsed() >= Duration::from_millis(100) {
            last_sent = Instant::now();
            let _ = on_progress.send(p);
        }
    };
    let result = sftp
        .download(&path, &sftp::download_dir(), &mut report, &cancel)
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
