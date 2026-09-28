//! In-app updates from GitHub Releases (tauri-plugin-updater).
//!
//! `update_check` looks for a newer release, `update_download` fetches its installer with
//! progress (the signature is verified against the public key built into the app) and
//! `update_install` runs it. On Windows the NSIS installer runs silently (`/S`, configured
//! in tauri.conf.json) and starts the new version when it is done.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use nexssh_core::i18n;
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

use crate::commands::{AppState, CmdError, CmdResult};

/// The update found by the last check, and its installer once downloaded.
#[derive(Default)]
pub struct Updates(Mutex<Option<Pending>>);

struct Pending {
    update: Update,
    installer: Option<Vec<u8>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    current_version: String,
    notes: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    downloaded: u64,
    total: Option<u64>,
}

/// Whether this build receives updates: releases are published for Windows only.
/// `NEXSSH_UPDATE_URL` points the updater at another manifest (testing, staging).
pub fn supported() -> bool {
    cfg!(windows) || override_url().is_some()
}

fn override_url() -> Option<String> {
    std::env::var("NEXSSH_UPDATE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
}

fn updater(app: &AppHandle) -> Result<Updater, String> {
    let mut builder = app.updater_builder();
    if let Some(url) = override_url() {
        let url = url.trim().parse().map_err(|e| format!("{e}"))?;
        builder = builder.endpoints(vec![url]).map_err(|e| e.to_string())?;
    }
    builder
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())
}

fn lock(updates: &Updates) -> std::sync::MutexGuard<'_, Option<Pending>> {
    updates.0.lock().unwrap_or_else(|e| e.into_inner())
}

#[tauri::command]
pub async fn update_check(
    app: AppHandle,
    updates: State<'_, Updates>,
) -> CmdResult<Option<UpdateInfo>> {
    let updater = updater(&app).map_err(|e| CmdError::from(i18n::update_check_failed(e)))?;
    let found = match updater.check().await {
        Ok(found) => found,
        // The latest release has no latest.json (e.g. it was published without the signing
        // key): there is nothing this app could install, so it has the newest version.
        Err(tauri_plugin_updater::Error::ReleaseNotFound) => None,
        Err(e) => return Err(CmdError::from(i18n::update_check_failed(e))),
    };
    let info = found.as_ref().map(|u| UpdateInfo {
        version: u.version.clone(),
        current_version: u.current_version.clone(),
        notes: u.body.clone().filter(|b| !b.trim().is_empty()),
    });
    let mut pending = lock(&updates);
    // Keep an already downloaded installer of the same version.
    let same = matches!((&*pending, &found), (Some(p), Some(u)) if p.update.version == u.version);
    if !same {
        *pending = found.map(|update| Pending {
            update,
            installer: None,
        });
    }
    Ok(info)
}

/// Downloads the installer of the update found by the last check.
#[tauri::command]
pub async fn update_download(
    updates: State<'_, Updates>,
    on_progress: Channel<DownloadProgress>,
) -> CmdResult<()> {
    let update = lock(&updates)
        .as_ref()
        .map(|p| p.update.clone())
        .ok_or_else(|| CmdError::from(i18n::update_missing()))?;

    let mut downloaded = 0u64;
    let mut total = None;
    let mut last_sent = Instant::now() - Duration::from_secs(1);
    let installer = update
        .download(
            |chunk, len| {
                downloaded += chunk as u64;
                total = len;
                if last_sent.elapsed() >= Duration::from_millis(100) {
                    last_sent = Instant::now();
                    let _ = on_progress.send(DownloadProgress { downloaded, total });
                }
            },
            || {},
        )
        .await
        .map_err(|e| CmdError::from(i18n::update_download_failed(e)))?;
    let _ = on_progress.send(DownloadProgress {
        downloaded,
        total: Some(downloaded),
    });

    if let Some(p) = lock(&updates).as_mut()
        && p.update.version == update.version
    {
        p.installer = Some(installer);
    }
    Ok(())
}

/// Installs the downloaded update and restarts. On Windows the installer runs without any
/// window and relaunches NexSSH itself; this process exits right away.
#[tauri::command]
pub fn update_install(
    app: AppHandle,
    state: State<'_, AppState>,
    updates: State<'_, Updates>,
) -> CmdResult<()> {
    // Outside Windows the updater replaces the running file as an AppImage: never let a
    // test manifest (NEXSSH_UPDATE_URL) overwrite a plain binary.
    if !cfg!(windows) && std::env::var_os("APPIMAGE").is_none() {
        return Err(CmdError::from(i18n::update_not_installable()));
    }
    let mut pending = lock(&updates);
    let Some(Pending {
        update,
        installer: Some(installer),
    }) = pending.as_ref()
    else {
        return Err(CmdError::from(i18n::update_not_downloaded()));
    };
    state.core.sessions.close_all();
    update
        .install(installer)
        .map_err(|e| CmdError::from(i18n::update_install_failed(e)))?;
    pending.take();
    drop(pending);
    // Windows never gets here (the updater exits); elsewhere start the new version.
    app.restart()
}
