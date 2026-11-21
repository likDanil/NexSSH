#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agents;
mod agents_ui;
mod app;
mod colors;
mod editor;
mod explorer;
mod files;
mod highlight;
mod i18n;
mod keyboard;
mod logger;
mod prompts;
mod server_editor;
mod services;
mod settings;
mod settings_schema;
mod terminal;
mod updates;

slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|a| a == "--verify-update") {
        let result = (|| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            if args.len() != 3 {
                return Err("Usage: --verify-update <manifest.json> <installer.exe>".into());
            }
            let release = serde_json::from_slice(&std::fs::read(&args[1])?)?;
            updates::verify(
                &std::fs::read(&args[2])?,
                &release,
                env!("NEXSSH_UPDATER_PUBLIC_KEY"),
            )
        })();
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    #[cfg(windows)]
    if let Some(code) = explorer::command(&std::env::args().skip(1).collect::<Vec<_>>()) {
        std::process::exit(code);
    }
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "mcp" || arg == "--mcp")
    {
        std::process::exit(agents::run_bridge());
    }
    app::run()
}
