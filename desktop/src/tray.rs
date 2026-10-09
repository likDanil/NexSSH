//! The icon in the notification area (the tray). With its window hidden there, NexSSH keeps
//! running: sessions stay connected, AI agents keep their access.
//!
//! The page decides what closing the window does, since it knows the settings and the tabs: hide
//! into the tray, quit (keeping the tabs for the next start), or ask. It also gives the menu, in
//! its language, and says when the icon shows: always, or only while the window is hidden. The
//! menu brings the window back, connects to a saved server, opens a local terminal and quits.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::image::Image;
use tauri::menu::{Menu, MenuBuilder, MenuEvent, MenuItem, Submenu, SubmenuBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, UserAttentionType, Wry};

const ID: &str = "main";
// Menu ids; every menu's clicks reach every listener, hence the prefix.
const OPEN: &str = "tray:open";
const LOCAL: &str = "tray:local";
const QUIT: &str = "tray:quit";
/// Followed by a saved server's id.
const SERVER: &str = "tray:server:";

/// The app's 32×32 icon: the window's is the largest picture of the .ico, which looks rough
/// scaled down that far.
const ICON: Image<'static> = tauri::include_image!("icons/32x32.png");

/// How long the page has to take a request to close: a page that is not loaded, or is stuck, must
/// not keep NexSSH from quitting.
const TAKE_WITHIN: Duration = Duration::from_secs(4);

/// Windows shows up to 127 characters of a tooltip.
const TOOLTIP_CHARS: usize = 100;

#[derive(Default)]
pub struct Tray {
    /// What the page wants in the tray; nothing until the page is up.
    spec: Mutex<Option<TraySpec>>,
    /// `spec` changed since the icon's menu was built.
    dirty: AtomicBool,
    /// The window is hidden into the tray.
    hidden: AtomicBool,
    /// Requests to close sent to the page, and the last one it took.
    asked: AtomicU64,
    taken: AtomicU64,
}

/// The tray as the page wants it.
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TraySpec {
    /// The icon shows while the window does too (the close button hides into the tray), not only
    /// while the window is hidden.
    always: bool,
    tooltip: String,
    /// Lines at the top of the menu, greyed out: open tabs, AI agents.
    status: Vec<String>,
    open: String,
    connect: String,
    /// Under `connect` when there are no saved servers.
    no_servers: String,
    local: String,
    quit: String,
    /// The saved servers by group, in the sidebar's order.
    groups: Vec<TrayGroup>,
}

#[derive(Deserialize, Clone)]
pub struct TrayGroup {
    /// Empty: the servers are listed in place, without a submenu.
    name: String,
    servers: Vec<TrayServer>,
}

#[derive(Deserialize, Clone)]
pub struct TrayServer {
    id: String,
    name: String,
}

/// What asks NexSSH to close: its window (the close button, Alt+F4, the taskbar), or "Quit" in
/// the tray's menu.
#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum Closing {
    Window,
    Quit,
}

#[derive(Serialize, Clone)]
struct CloseRequest {
    id: u64,
    kind: Closing,
}

/// Something the page does once the tray brought the window back.
#[derive(Serialize, Clone)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum TrayAction {
    Connect { server_id: String },
    Local,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// This system can show the icon.
#[cfg(not(target_os = "linux"))]
pub fn supported() -> bool {
    true
}

/// This system can show the icon: it takes libayatana-appindicator (or the older
/// libappindicator), which tray-icon loads when it creates the icon, and panics without.
#[cfg(target_os = "linux")]
pub fn supported() -> bool {
    static FOUND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| {
        ["libayatana-appindicator3.so.1", "libappindicator3.so.1"]
            .iter()
            .any(|name| {
                // SAFETY: the library tray-icon loads itself; it stays loaded for it.
                match unsafe { libloading::Library::new(name) } {
                    Ok(library) => {
                        std::mem::forget(library);
                        true
                    }
                    Err(_) => false,
                }
            })
    })
}

/// Listens to the menu's clicks, once: every icon created later uses the same listener.
pub fn init(app: &tauri::App) {
    app.on_menu_event(on_menu);
}

/// Leaves closing to the page, which knows the settings and the tabs: it hides the window into
/// the tray, quits or asks the user. A page that does not take the request in time does not keep
/// NexSSH from quitting.
pub fn request_close(app: &AppHandle, kind: Closing) {
    let id = app.state::<Tray>().asked.fetch_add(1, Ordering::SeqCst) + 1;
    if let Err(e) = app.emit("close-request", CloseRequest { id, kind }) {
        log::warn!("cannot ask the page about closing: {e}");
        app.exit(0);
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(TAKE_WITHIN).await;
        if app.state::<Tray>().taken.load(Ordering::SeqCst) < id {
            log::warn!("the page did not take the request to close: quitting");
            app.exit(0);
        }
    });
}

/// Brings the window to the front: shown if it was hidden in the tray, restored if minimized.
pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    if app.state::<Tray>().hidden.swap(false, Ordering::SeqCst) {
        update(app);
    }
}

/// Hides the window into the tray. The icon comes first: without it (no menu from the page yet,
/// or the system has no tray) the window stays, since nothing could bring it back.
fn hide_window(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let tray = handle.state::<Tray>();
        tray.hidden.store(true, Ordering::SeqCst);
        apply(&handle);
        if handle.tray_by_id(ID).is_none() {
            tray.hidden.store(false, Ordering::SeqCst);
            return;
        }
        if let Some(window) = handle.get_webview_window("main") {
            let _ = window.hide();
        }
    });
}

/// The user is needed (an AI agent's question, a password for a tab opened for an agent): the
/// window comes back from the tray, and its taskbar button flashes (the dock icon bounces on
/// macOS) unless it has the focus.
pub fn attention(app: &AppHandle) {
    if app.state::<Tray>().hidden.load(Ordering::SeqCst) {
        show_window(app);
    }
    if let Some(window) = app.get_webview_window("main")
        && !window.is_focused().unwrap_or(false)
    {
        let _ = window.request_user_attention(Some(UserAttentionType::Informational));
    }
}

/// Takes the icon away now: the updater ends the process without the usual clean-up, which would
/// leave the icon in the tray until the mouse passes over it.
pub fn remove(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = handle.remove_tray_by_id(ID);
    });
}

/// Puts the icon in line with the page's menu and the window, on the main thread: one call after
/// another, so the icon is created once.
fn update(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || apply(&handle));
}

/// The icon is there with the latest menu while it should show (always, or while the window is
/// hidden), and hidden otherwise. Only on the main thread.
fn apply(app: &AppHandle) {
    let tray = app.state::<Tray>();
    // Before reading the spec: a newer one sets it again, and its own update builds the menu.
    let rebuild = tray.dirty.swap(false, Ordering::SeqCst);
    let Some(spec) = lock(&tray.spec).clone() else {
        return;
    };
    let visible = spec.always || tray.hidden.load(Ordering::SeqCst);
    let result = match app.tray_by_id(ID) {
        Some(icon) => refresh(app, &icon, &spec, rebuild, visible),
        None if visible => create(app, &spec).map(drop),
        None => Ok(()),
    };
    if let Err(e) = result {
        log::warn!("tray icon: {e}");
    }
}

fn refresh(
    app: &AppHandle,
    icon: &TrayIcon<Wry>,
    spec: &TraySpec,
    rebuild: bool,
    visible: bool,
) -> tauri::Result<()> {
    if rebuild {
        icon.set_menu(Some(menu(app, spec)?))?;
        icon.set_tooltip(Some(tooltip(spec)))?;
    }
    icon.set_visible(visible)
}

fn create(app: &AppHandle, spec: &TraySpec) -> tauri::Result<TrayIcon<Wry>> {
    TrayIconBuilder::with_id(ID)
        .icon(ICON)
        .tooltip(tooltip(spec))
        .menu(&menu(app, spec)?)
        // The menu is on the right button; the left one brings the window back.
        .show_menu_on_left_click(false)
        .on_tray_icon_event(on_icon)
        .build(app)
}

/// The state on top (greyed out), the window, the servers, a local terminal, and quitting.
fn menu(app: &AppHandle, spec: &TraySpec) -> tauri::Result<Menu<Wry>> {
    let mut menu = MenuBuilder::new(app);
    for (i, line) in spec.status.iter().enumerate() {
        let item = MenuItem::with_id(
            app,
            format!("tray:status:{i}"),
            label(line),
            false,
            None::<&str>,
        )?;
        menu = menu.item(&item);
    }
    if !spec.status.is_empty() {
        menu = menu.separator();
    }
    menu.text(OPEN, label(&spec.open))
        .item(&servers(app, spec)?)
        .text(LOCAL, label(&spec.local))
        .separator()
        .text(QUIT, label(&spec.quit))
        .build()
}

/// The saved servers by group, as in the sidebar; a group without a name lists its servers in
/// place.
fn servers(app: &AppHandle, spec: &TraySpec) -> tauri::Result<Submenu<Wry>> {
    let mut connect = SubmenuBuilder::new(app, label(&spec.connect));
    let mut empty = true;
    for group in spec.groups.iter().filter(|g| !g.servers.is_empty()) {
        empty = false;
        if group.name.is_empty() {
            for server in &group.servers {
                connect = connect.text(format!("{SERVER}{}", server.id), label(&server.name));
            }
        } else {
            let mut submenu = SubmenuBuilder::new(app, label(&group.name));
            for server in &group.servers {
                submenu = submenu.text(format!("{SERVER}{}", server.id), label(&server.name));
            }
            connect = connect.item(&submenu.build()?);
        }
    }
    if empty {
        let none = MenuItem::with_id(
            app,
            "tray:servers:none",
            label(&spec.no_servers),
            false,
            None::<&str>,
        )?;
        connect = connect.item(&none);
    }
    connect.build()
}

/// A menu text as it is: `&` would make the next letter the item's access key.
fn label(text: &str) -> String {
    text.replace('&', "&&")
}

fn tooltip(spec: &TraySpec) -> String {
    let mut text: String = spec.tooltip.chars().take(TOOLTIP_CHARS).collect();
    if text.len() < spec.tooltip.len() {
        text.push('…');
    }
    text
}

fn on_icon(icon: &TrayIcon<Wry>, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        show_window(icon.app_handle());
    }
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        OPEN => show_window(app),
        QUIT => request_close(app, Closing::Quit),
        LOCAL => act(app, TrayAction::Local),
        id => {
            if let Some(server_id) = id.strip_prefix(SERVER) {
                let server_id = server_id.to_string();
                act(app, TrayAction::Connect { server_id });
            }
        }
    }
}

/// The window comes back, and the page does the rest.
fn act(app: &AppHandle, action: TrayAction) {
    show_window(app);
    let _ = app.emit("tray", action);
}

// ---- commands ---------------------------------------------------------------------

/// The page's tray: the menu, in its language, and whether the icon shows while the window does.
#[tauri::command]
pub fn tray_set(app: AppHandle, tray: State<'_, Tray>, spec: TraySpec) {
    if !supported() {
        return;
    }
    *lock(&tray.spec) = Some(spec);
    tray.dirty.store(true, Ordering::SeqCst);
    update(&app);
}

/// The page took a request to close; it decides from here on.
#[tauri::command]
pub fn close_taken(tray: State<'_, Tray>, id: u64) {
    tray.taken.fetch_max(id, Ordering::SeqCst);
}

/// Hides the window into the tray (it stays when there is no icon to bring it back with).
#[tauri::command]
pub fn window_hide(app: AppHandle) {
    hide_window(&app);
}

#[tauri::command]
pub fn window_show(app: AppHandle) {
    show_window(&app);
}

/// The user is needed in a tab (a password for a tab an AI agent opened).
#[tauri::command]
pub fn window_attention(app: AppHandle) {
    attention(&app);
}

/// Quits: the page has decided, and kept the tabs for the next start if it should.
#[tauri::command]
pub fn app_exit(app: AppHandle) {
    app.exit(0);
}
