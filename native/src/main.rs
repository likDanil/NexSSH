#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agents;
mod agents_ui;
mod app;
mod colors;
mod editor;
mod explorer;
mod files;
mod fonts;
mod highlight;
mod i18n;
mod instance;
mod keyboard;
mod logger;
mod metrics;
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
    if let Some(code) = explorer::command(&args) {
        std::process::exit(code);
    }
    if args
        .first()
        .is_some_and(|arg| arg == "mcp" || arg == "--mcp")
    {
        std::process::exit(agents::run_bridge());
    }
    let data_dir = nexssh_core::default_data_dir().ok_or("Не найдена папка данных NexSSH")?;
    let launch = instance::Launch::from_args(&args, &std::env::current_dir()?)?;
    match instance::start(&data_dir, &launch)? {
        instance::Start::Forwarded => Ok(()),
        instance::Start::Primary(primary) => app::run(data_dir, primary, launch),
    }
}
