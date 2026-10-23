//! Settings → Backup: everything NexSSH keeps saved into one file, and restored from one
//! (see `nexssh_core::backup` for what goes in).

use std::path::PathBuf;

use nexssh_core::backup::{self, Restored, Summary};
use nexssh_core::i18n;
use serde::Serialize;
use serde_json::{Map, Value};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::agents::Agents;
use crate::commands::{AppState, CmdError, CmdResult, blocking};
use crate::edit::Editing;

/// Settings of this computer and this run, not of the user: kept out of backups.
const LOCAL_SETTINGS: [&str; 2] = ["lastTabs", "lastVersion"];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exported {
    path: String,
    /// Saved passwords that went in.
    passwords: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    path: String,
    summary: Summary,
}

/// Asks where to save a backup and writes it there. With `password`, the saved passwords go
/// in too, encrypted with it. `None` when the dialog was cancelled.
#[tauri::command]
pub async fn backup_export(
    window: tauri::WebviewWindow,
    title: String,
    file_name: String,
    password: Option<String>,
) -> CmdResult<Option<Exported>> {
    let dialog = window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(title)
        .set_file_name(file_name)
        .add_filter(i18n::backup_filter(), &["json"]);
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
    let app = window.app_handle().clone();
    blocking(move || {
        let state = app.state::<AppState>();
        let mut settings = state
            .settings
            .lock()
            .map_err(|e| e.to_string())?
            .value()
            .clone();
        if let Some(map) = settings.as_object_mut() {
            for key in LOCAL_SETTINGS {
                map.remove(key);
            }
        }
        let password = password.filter(|p| !p.is_empty());
        let created = backup::create(&state.core, settings, password.as_deref())?;
        std::fs::write(&path, &created.json)?;
        log::info!("backup written to {}", path.display());
        Ok(Some(Exported {
            path: path.display().to_string(),
            passwords: created.passwords,
        }))
    })
    .await?
}

/// Asks for a backup file and tells what is inside; `None` when the dialog was cancelled.
#[tauri::command]
pub async fn backup_open(window: tauri::WebviewWindow, title: String) -> CmdResult<Option<Opened>> {
    let dialog = window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(title)
        .add_filter(i18n::backup_filter(), &["json"]);
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
    blocking(move || {
        let backup = backup::parse(&std::fs::read(&path)?)?;
        Ok(Some(Opened {
            path: path.display().to_string(),
            summary: backup.summary(),
        }))
    })
    .await?
}

/// Restores the backup at `path`: its servers and settings replace the ones here. With the
/// backup's `password` its saved passwords are restored too; a wrong one changes nothing.
/// Sessions and edited files are closed: the page loads again afterwards.
#[tauri::command]
pub async fn backup_restore(
    app: AppHandle,
    path: String,
    password: Option<String>,
) -> CmdResult<Restored> {
    blocking(move || {
        let backup = backup::parse(&std::fs::read(PathBuf::from(&path))?)?;
        let passwords = match password.filter(|p| !p.is_empty()) {
            Some(p) => backup.open_passwords(&p)?,
            None => Default::default(),
        };

        let state = app.state::<AppState>();
        let agents = app.state::<Agents>();
        state.core.sessions.close_all();
        app.state::<Editing>().0.stop_all();
        for server in state.core.store.snapshot().servers {
            agents.forget_server(&server.id);
        }
        let restored = backup::restore(&state.core, &backup, &passwords)?;

        let mut settings = state.settings.lock().map_err(|e| e.to_string())?;
        let mut next = match &backup.settings {
            Value::Object(map) => map.clone(),
            _ => Map::new(),
        };
        for key in LOCAL_SETTINGS {
            next.remove(key);
        }
        // The news of this version were shown here already; the tabs of the last run were of
        // the servers that are gone.
        if let Some(version) = settings.value().get("lastVersion") {
            next.insert("lastVersion".into(), version.clone());
        }
        let next = Value::Object(next);
        settings.save(next.clone())?;
        drop(settings);
        agents.apply(&next);
        log::info!(
            "backup {path} restored: {} servers, {} passwords",
            restored.servers,
            restored.passwords
        );
        Ok(restored)
    })
    .await?
}
