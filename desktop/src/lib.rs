//! NexSSH desktop shell: creates the window and exposes the core over Tauri IPC.

mod commands;
#[cfg(windows)]
mod explorer;
mod local;
mod logger;
mod settings;
mod sftp;
mod sink;
mod updates;

use std::sync::Mutex;

use tauri::window::Color;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

use commands::AppState;
use settings::{Settings, theme_background};

pub fn run() {
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().collect();
        // Explorer menu tasks, without a window: the uninstaller removes the entries through
        // the app (windows/hooks.nsh), and the steps that need administrator rights run in a
        // copy of NexSSH that Windows started with them (explorer.rs).
        match args.get(1).map(String::as_str) {
            Some("--explorer-cleanup") => return explorer::uninstall(),
            Some("--explorer-trust") => std::process::exit(explorer::trust_computer()),
            Some("--explorer-untrust") => std::process::exit(explorer::untrust_computer()),
            _ => {}
        }
        // Started from Explorer's menu while NexSSH runs: this start hands its folder over,
        // and the running window may come to the front.
        if args.iter().any(|a| a.starts_with("--cwd")) {
            nexssh_explorer::allow_foreground();
        }
    }
    logger::init();
    let app = tauri::Builder::default()
        // First, so a second launch hands over before creating anything: the running
        // NexSSH comes to the front instead of a second copy opening next to it (two copies
        // would also overwrite each other's servers.json and settings.json).
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            log::info!("NexSSH was started again: showing this window");
            // `--cwd <folder>` (Explorer's "Open with NexSSH") opens a local terminal here.
            local::handle_second_start(app, &args, &cwd);
            show_main_window(app);
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Only its Rust API is used (the key file picker); the webview gets no dialog permissions.
        .plugin(tauri_plugin_dialog::init())
        .manage(updates::Updates::default())
        .manage(sftp::Transfers::default())
        .manage(local::Launches::default())
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Enter { paths, .. }) => {
                sftp::dragged(window, paths)
            }
            tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, position }) => {
                sftp::dropped(window, paths, *position)
            }
            _ => {}
        })
        .setup(|app| {
            let data_dir = nexssh_core::default_data_dir()
                .ok_or("cannot determine the configuration directory")?;
            let core = nexssh_core::Core::open(&data_dir)?;
            let settings = Settings::load(&data_dir);
            let (r, g, b) = theme_background(settings.theme(), false);
            let args: Vec<String> = std::env::args().collect();
            let cwd = std::env::current_dir().unwrap_or_default();
            app.state::<local::Launches>().add(&args, &cwd);
            app.manage(AppState {
                core,
                settings: Mutex::new(settings),
                restore_maximized: Default::default(),
            });
            create_main_window(app, Color(r, g, b, 255))?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::app_ready,
            commands::app_set_language,
            commands::window_set_fullscreen,
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
            commands::pick_key_file,
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
            local::local_shells,
            local::local_open,
            local::pick_program,
            local::explorer_menu,
            local::explorer_menu_trust,
            local::launch_take,
            sftp::sftp_home,
            sftp::sftp_resolve,
            sftp::sftp_list,
            sftp::sftp_mkdir,
            sftp::sftp_ensure_dir,
            sftp::sftp_new_file,
            sftp::sftp_chmod,
            sftp::sftp_rename,
            sftp::sftp_remove,
            sftp::sftp_download,
            sftp::sftp_pick_destination,
            sftp::sftp_pick_upload,
            sftp::sftp_upload_path,
            sftp::sftp_upload_begin,
            sftp::sftp_upload_chunk,
            sftp::sftp_upload_end,
            sftp::sftp_cancel,
            sftp::reveal_download,
            updates::update_check,
            updates::update_download,
            updates::update_install,
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
/// Brings the main window to the front, restored if it was minimized.
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn create_main_window(app: &tauri::App, background: Color) -> tauri::Result<()> {
    let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
        .title("NexSSH")
        .inner_size(1180.0, 760.0)
        .min_inner_size(640.0, 400.0)
        .center()
        .background_color(background)
        // navigator.clipboard for copy/paste in the terminal (no clipboard plugin needed).
        .enable_clipboard_access();

    // The page handles drag & drop itself (tabs, files dropped on the files drawer): the
    // native handler would take every drag from WebView2 and WKWebView. WebKitGTK keeps
    // the page's own drags working next to it but never gives dropped files to the page,
    // so on Linux the native handler stays and tells the page what is being dragged in
    // (`sftp::dragged`).
    #[cfg(not(target_os = "linux"))]
    let builder = builder.disable_drag_drop_handler();

    #[cfg(target_os = "windows")]
    let builder = builder.decorations(false).shadow(true);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    builder.build()?;
    Ok(())
}
