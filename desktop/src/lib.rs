//! NexSSH desktop shell: creates the window and exposes the core over Tauri IPC.

mod commands;
mod logger;
mod settings;
mod sink;

use std::sync::Mutex;

use tauri::window::Color;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

use commands::AppState;
use settings::{Settings, theme_background};

pub fn run() {
    logger::init();
    let app = tauri::Builder::default()
        .setup(|app| {
            let data_dir = nexssh_core::default_data_dir()
                .ok_or("cannot determine the configuration directory")?;
            let core = nexssh_core::Core::open(&data_dir)?;
            let settings = Settings::load(&data_dir);
            let (r, g, b) = theme_background(settings.theme(), false);
            app.manage(AppState {
                core,
                settings: Mutex::new(settings),
            });
            create_main_window(app, Color(r, g, b, 255))?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::app_ready,
            commands::keychain_status,
            commands::settings_get,
            commands::settings_set,
            commands::servers_list,
            commands::server_save,
            commands::server_delete,
            commands::server_has_password,
            commands::groups_set,
            commands::group_rename,
            commands::group_delete,
            commands::ssh_config_import,
            commands::keys_list,
            commands::session_open,
            commands::session_write,
            commands::session_write_binary,
            commands::session_resize,
            commands::session_reconnect,
            commands::session_disconnect,
            commands::session_close,
            commands::prompt_answer,
            commands::forward_add,
            commands::forward_remove,
        ])
        .build(tauri::generate_context!())
        .expect("failed to start NexSSH");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            handle.state::<AppState>().core.sessions.close_all();
        }
    });
}

/// The window is created in code (not in tauri.conf.json) so its background matches
/// the saved theme before the page paints, and so title bars can differ per platform:
/// Windows gets a custom title bar drawn by the UI, macOS keeps its traffic lights over
/// the content, Linux keeps native decorations.
fn create_main_window(app: &tauri::App, background: Color) -> tauri::Result<()> {
    let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
        .title("NexSSH")
        .inner_size(1180.0, 760.0)
        .min_inner_size(640.0, 400.0)
        .center()
        .background_color(background)
        // Let the page handle drag & drop itself (tab reordering).
        .disable_drag_drop_handler()
        // navigator.clipboard for copy/paste in the terminal (no clipboard plugin needed).
        .enable_clipboard_access();

    #[cfg(target_os = "windows")]
    let builder = builder.decorations(false).shadow(true);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    builder.build()?;
    Ok(())
}
