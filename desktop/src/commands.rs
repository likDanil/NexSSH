//! Tauri commands: the whole IPC surface between the webview and the Rust core.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use nexssh_core::i18n::{self, Lang};
use nexssh_core::keys::KeyInfo;
use nexssh_core::secrets::Secrets;
use nexssh_core::{
    Core, Destination, ForwardSpec, ImportReport, PromptReply, PtySize, Server, SessionId,
    StoreData,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;
use tauri::ipc::{Channel, InvokeResponseBody};
use zeroize::Zeroizing;

use crate::agents::Agents;
use crate::edit::Editing;
use crate::settings::Settings;
use crate::sink::ChannelSink;

pub struct AppState {
    pub core: Core,
    pub settings: Mutex<Settings>,
    /// The window was maximized before entering full screen.
    pub restore_maximized: AtomicBool,
}

/// Errors cross the IPC boundary as plain strings, ready to be shown to the user.
#[derive(Debug)]
pub struct CmdError(String);

impl<E: std::fmt::Display> From<E> for CmdError {
    fn from(e: E) -> Self {
        CmdError(e.to_string())
    }
}

impl Serialize for CmdError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

pub type CmdResult<T> = Result<T, CmdError>;

fn err<T>(msg: impl Into<String>) -> CmdResult<T> {
    Err(CmdError(msg.into()))
}

pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> CmdResult<T> {
    Ok(tauri::async_runtime::spawn_blocking(f).await?)
}

// ---- app --------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: &'static str,
    os: String,
    data_dir: String,
    settings: Value,
    /// This build can update itself (see `updates.rs`).
    updates: bool,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    let settings = state
        .settings
        .lock()
        .map(|s| s.value().clone())
        .unwrap_or(Value::Null);
    // NEXSSH_UI_OS lets contributors preview another platform's window chrome.
    let os = std::env::var("NEXSSH_UI_OS").unwrap_or_else(|_| std::env::consts::OS.to_string());
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        os,
        data_dir: state.core.data_dir.display().to_string(),
        settings,
        updates: crate::updates::supported(),
    }
}

/// Called when the page (re)loads: sessions of a previous page instance are orphaned.
#[tauri::command]
pub fn app_ready(state: State<'_, AppState>, editing: State<'_, Editing>) {
    state.core.sessions.close_all();
    editing.0.stop_all();
}

/// Language of messages produced by the backend (errors, connection progress).
/// The UI resolves "system" to a concrete language and calls this at start-up and on
/// every change; unknown tags fall back to English.
#[tauri::command]
pub fn app_set_language(lang: String) {
    i18n::set_language(Lang::from_tag(&lang).unwrap_or_default());
}

/// `None` when passwords can be saved, otherwise why not.
#[tauri::command]
pub async fn keychain_status(state: State<'_, AppState>) -> CmdResult<Option<String>> {
    let secrets = Arc::clone(&state.core.secrets);
    blocking(move || secrets.unavailable_reason()).await
}

/// Enters or leaves full screen.
///
/// On Windows a maximized borderless window keeps the monitor's work area as its client
/// area even in full screen (tao checks "maximized" before "fullscreen" when sizing the
/// client area), which leaves a strip at the bottom where the taskbar is. So the window
/// leaves the maximized state first and gets it back when full screen ends.
#[tauri::command]
pub async fn window_set_fullscreen(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    fullscreen: bool,
) -> CmdResult<()> {
    if window.is_fullscreen()? == fullscreen {
        return Ok(());
    }
    if fullscreen {
        let maximized = cfg!(windows) && window.is_maximized()?;
        state.restore_maximized.store(maximized, Ordering::Relaxed);
        if maximized {
            window.unmaximize()?;
        }
        window.set_fullscreen(true)?;
    } else {
        window.set_fullscreen(false)?;
        if state.restore_maximized.swap(false, Ordering::Relaxed) {
            window.maximize()?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> CmdResult<Value> {
    Ok(state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .value()
        .clone())
}

#[tauri::command]
pub async fn settings_set(
    state: State<'_, AppState>,
    agents: State<'_, Agents>,
    settings: Value,
) -> CmdResult<()> {
    state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .save(settings.clone())?;
    agents.apply(&settings);
    Ok(())
}

// ---- servers ----------------------------------------------------------------------

#[tauri::command]
pub fn servers_list(state: State<'_, AppState>) -> StoreData {
    state.core.store.snapshot()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    server: Server,
    /// Set when the server was saved but its password could not be stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
}

/// Creates or updates a server. A non-empty `password` (or `jump_password`, for its jump
/// host) is stored in the OS keychain; `clear_password` (`clear_jump_password`) removes a
/// stored one. Keychain problems never lose the server: they are reported as a warning.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn server_save(
    state: State<'_, AppState>,
    agents: State<'_, Agents>,
    server: Server,
    password: Option<String>,
    clear_password: Option<bool>,
    jump_password: Option<String>,
    clear_jump_password: Option<bool>,
) -> CmdResult<SaveResult> {
    let before = state.core.store.get(&server.id).map(|s| s.agents);
    let saved = state.core.store.save_server(server)?;
    if before != Some(saved.agents) {
        agents.forget_server(&saved.id);
    }
    let secrets = &state.core.secrets;
    let own = Secrets::password_account(&saved.id);
    let jump = Secrets::jump_password_account(&saved.id);
    let result = match store_secret(secrets, own, password, clear_password).await {
        Ok(()) => store_secret(secrets, jump, jump_password, clear_jump_password).await,
        Err(e) => Err(e),
    };
    let warning = result.err().map(i18n::password_not_stored);
    Ok(SaveResult {
        server: saved,
        warning,
    })
}

/// Stores a non-empty `secret` under `account`, or removes the stored one on `clear`.
async fn store_secret(
    secrets: &Arc<Secrets>,
    account: String,
    secret: Option<String>,
    clear: Option<bool>,
) -> nexssh_core::Result<()> {
    match secret.filter(|p| !p.is_empty()) {
        Some(p) => secrets.set_async(account, Zeroizing::new(p)).await,
        None if clear == Some(true) => {
            let secrets = Arc::clone(secrets);
            blocking(move || secrets.delete(&account))
                .await
                .unwrap_or_else(|e| Err(nexssh_core::Error::Secret(e.0)))
        }
        None => Ok(()),
    }
}

#[tauri::command]
pub async fn server_delete(
    state: State<'_, AppState>,
    agents: State<'_, Agents>,
    id: String,
) -> CmdResult<()> {
    state.core.store.delete_server(&id)?;
    agents.forget_server(&id);
    let secrets = Arc::clone(&state.core.secrets);
    blocking(move || {
        secrets.delete(&Secrets::password_account(&id))?;
        secrets.delete(&Secrets::jump_password_account(&id))
    })
    .await??;
    Ok(())
}

/// Whether a password is remembered for the server, or with `jump` for its jump host.
#[tauri::command]
pub async fn server_has_password(
    state: State<'_, AppState>,
    id: String,
    jump: Option<bool>,
) -> CmdResult<bool> {
    let account = if jump == Some(true) {
        Secrets::jump_password_account(&id)
    } else {
        Secrets::password_account(&id)
    };
    let secrets = Arc::clone(&state.core.secrets);
    blocking(move || secrets.has(&account)).await
}

#[tauri::command]
pub async fn groups_set(state: State<'_, AppState>, groups: Vec<String>) -> CmdResult<StoreData> {
    state.core.store.set_groups(groups)?;
    Ok(state.core.store.snapshot())
}

#[tauri::command]
pub async fn group_rename(
    state: State<'_, AppState>,
    from: String,
    to: String,
) -> CmdResult<StoreData> {
    state.core.store.rename_group(&from, &to)?;
    Ok(state.core.store.snapshot())
}

#[tauri::command]
pub async fn group_delete(state: State<'_, AppState>, name: String) -> CmdResult<StoreData> {
    state.core.store.remove_group(&name)?;
    Ok(state.core.store.snapshot())
}

#[tauri::command]
pub async fn ssh_config_import(
    state: State<'_, AppState>,
    path: Option<String>,
) -> CmdResult<ImportReport> {
    let path = path
        .filter(|p| !p.trim().is_empty())
        .map(|p| nexssh_core::expand_tilde(p.trim()));
    Ok(state
        .core
        .import_ssh_config(path.as_deref(), "SSH config")?)
}

#[tauri::command]
pub async fn keys_list() -> CmdResult<Vec<KeyInfo>> {
    blocking(nexssh_core::keys::list_local_keys).await
}

/// Opens the system file dialog in `~/.ssh` and returns the chosen private key, written like
/// the other key paths in the editor (`~/…`); `None` when cancelled.
#[tauri::command]
pub async fn pick_key_file(
    window: tauri::WebviewWindow,
    title: String,
) -> CmdResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;

    let mut dialog = window.dialog().file().set_parent(&window).set_title(title);
    if let Some(dir) = nexssh_core::ssh_dir().filter(|d| d.is_dir()) {
        dialog = dialog.set_directory(dir);
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
    Ok(Some(nexssh_core::contract_tilde(&private_key_for(&path))))
}

/// The system folder dialog, opened in `start`; `None` when cancelled.
pub(crate) async fn choose_folder(
    window: &tauri::WebviewWindow,
    title: String,
    start: std::path::PathBuf,
) -> CmdResult<Option<std::path::PathBuf>> {
    use tauri_plugin_dialog::DialogExt;

    let dialog = window
        .dialog()
        .file()
        .set_parent(window)
        .set_title(title)
        .set_directory(start);
    // The dialog runs on the main thread; wait for its answer here.
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    dialog.pick_folder(move |picked| {
        let _ = tx.try_send(picked);
    });
    let Some(picked) = rx.recv().await.flatten() else {
        return Ok(None);
    };
    picked
        .into_path()
        .map(Some)
        .map_err(|e| CmdError::from(e.to_string()))
}

/// Asks for any number of local files; empty when cancelled.
pub(crate) async fn choose_files(
    window: &tauri::WebviewWindow,
    title: String,
    start: std::path::PathBuf,
) -> CmdResult<Vec<std::path::PathBuf>> {
    use tauri_plugin_dialog::DialogExt;

    let dialog = window
        .dialog()
        .file()
        .set_parent(window)
        .set_title(title)
        .set_directory(start);
    // The dialog runs on the main thread; wait for its answer here.
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    dialog.pick_files(move |picked| {
        let _ = tx.try_send(picked);
    });
    rx.recv()
        .await
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .map(|picked| {
            picked
                .into_path()
                .map_err(|e| CmdError::from(e.to_string()))
        })
        .collect()
}

/// A picked `.pub` file stands for the private key next to it.
fn private_key_for(path: &std::path::Path) -> std::path::PathBuf {
    if path.extension().is_some_and(|e| e == "pub") {
        let private = path.with_extension("");
        if private.is_file() {
            return private;
        }
    }
    path.to_path_buf()
}

// ---- sessions ---------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTarget {
    server_id: Option<String>,
    /// Quick connect: `user@host:port`, or the name/alias of a saved server.
    destination: Option<String>,
}

#[tauri::command]
pub async fn session_open(
    state: State<'_, AppState>,
    target: OpenTarget,
    cols: u32,
    rows: u32,
    on_event: Channel<InvokeResponseBody>,
) -> CmdResult<SessionId> {
    let store = &state.core.store;
    let server = match (target.server_id, target.destination) {
        (Some(id), _) => match store.get(&id) {
            Some(s) => s,
            None => return err(i18n::server_missing()),
        },
        (None, Some(dest)) => match store.find(dest.trim()) {
            Some(s) => s,
            None => Destination::parse(&dest)?.to_server(),
        },
        (None, None) => return err(i18n::nothing_to_connect()),
    };
    let sink = Arc::new(ChannelSink::new(on_event));
    Ok(state
        .core
        .sessions
        .open(server, PtySize::new(cols, rows), sink))
}

#[tauri::command]
pub fn session_write(state: State<'_, AppState>, id: SessionId, data: String) -> CmdResult<()> {
    Ok(state.core.sessions.write(id, data.into_bytes())?)
}

#[tauri::command]
pub fn session_write_binary(
    state: State<'_, AppState>,
    id: SessionId,
    data: Vec<u8>,
) -> CmdResult<()> {
    Ok(state.core.sessions.write(id, data)?)
}

#[tauri::command]
pub fn session_resize(
    state: State<'_, AppState>,
    id: SessionId,
    cols: u32,
    rows: u32,
) -> CmdResult<()> {
    Ok(state.core.sessions.resize(id, PtySize::new(cols, rows))?)
}

#[tauri::command]
pub fn session_reconnect(state: State<'_, AppState>, id: SessionId) -> CmdResult<()> {
    Ok(state.core.sessions.reconnect(id)?)
}

#[tauri::command]
pub fn session_disconnect(state: State<'_, AppState>, id: SessionId) -> CmdResult<()> {
    Ok(state.core.sessions.disconnect(id)?)
}

/// Closes a session; the files edited on it are no longer sent.
#[tauri::command]
pub fn session_close(state: State<'_, AppState>, editing: State<'_, Editing>, id: SessionId) {
    state.core.sessions.close(id);
    editing.0.stop_session(id);
}

#[tauri::command]
pub fn prompt_answer(state: State<'_, AppState>, id: u64, reply: PromptReply) -> CmdResult<()> {
    Ok(state.core.sessions.answer(id, reply)?)
}

// ---- port forwarding --------------------------------------------------------------

/// Starts a forward on a live session; with `save_to` it is also added to that
/// server's saved forwards (started automatically on every connection).
#[tauri::command]
pub async fn forward_add(
    state: State<'_, AppState>,
    session_id: SessionId,
    spec: ForwardSpec,
    save_to: Option<String>,
) -> CmdResult<()> {
    state
        .core
        .sessions
        .add_forward(session_id, spec.clone())
        .await?;
    if let Some(id) = save_to
        && let Some(mut server) = state.core.store.get(&id)
        && !server.forwards.contains(&spec)
    {
        server.forwards.push(spec);
        state.core.store.save_server(server)?;
    }
    Ok(())
}

#[tauri::command]
pub fn forward_remove(
    state: State<'_, AppState>,
    session_id: SessionId,
    forward_id: u64,
) -> CmdResult<()> {
    Ok(state.core.sessions.remove_forward(session_id, forward_id)?)
}
