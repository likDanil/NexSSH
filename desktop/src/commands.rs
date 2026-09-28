//! Tauri commands: the whole IPC surface between the webview and the Rust core.

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

use crate::settings::Settings;
use crate::sink::ChannelSink;

pub struct AppState {
    pub core: Core,
    pub settings: Mutex<Settings>,
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

type CmdResult<T> = Result<T, CmdError>;

fn err<T>(msg: impl Into<String>) -> CmdResult<T> {
    Err(CmdError(msg.into()))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> CmdResult<T> {
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
    }
}

/// Called when the page (re)loads: sessions of a previous page instance are orphaned.
#[tauri::command]
pub fn app_ready(state: State<'_, AppState>) {
    state.core.sessions.close_all();
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
pub async fn settings_set(state: State<'_, AppState>, settings: Value) -> CmdResult<()> {
    state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .save(settings)?;
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

/// Creates or updates a server. A non-empty `password` is stored in the OS keychain;
/// `clear_password` removes a stored one. Keychain problems never lose the server:
/// they are reported as a warning.
#[tauri::command]
pub async fn server_save(
    state: State<'_, AppState>,
    server: Server,
    password: Option<String>,
    clear_password: Option<bool>,
) -> CmdResult<SaveResult> {
    let saved = state.core.store.save_server(server)?;
    let account = Secrets::password_account(&saved.id);
    let secrets = Arc::clone(&state.core.secrets);
    let result = match password.filter(|p| !p.is_empty()) {
        Some(p) => secrets.set_async(account, Zeroizing::new(p)).await,
        None if clear_password == Some(true) => blocking(move || secrets.delete(&account))
            .await
            .unwrap_or_else(|e| Err(nexssh_core::Error::Secret(e.0))),
        None => Ok(()),
    };
    let warning = result.err().map(i18n::password_not_stored);
    Ok(SaveResult {
        server: saved,
        warning,
    })
}

#[tauri::command]
pub async fn server_delete(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.core.store.delete_server(&id)?;
    let secrets = Arc::clone(&state.core.secrets);
    blocking(move || secrets.delete(&Secrets::password_account(&id))).await??;
    Ok(())
}

#[tauri::command]
pub async fn server_has_password(state: State<'_, AppState>, id: String) -> CmdResult<bool> {
    let secrets = Arc::clone(&state.core.secrets);
    blocking(move || secrets.has(&Secrets::password_account(&id))).await
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

#[tauri::command]
pub fn session_close(state: State<'_, AppState>, id: SessionId) {
    state.core.sessions.close(id);
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
