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
mod logger;
mod prompts;
mod services;
mod settings;
mod settings_schema;
mod terminal;

slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "mcp" || arg == "--mcp")
    {
        std::process::exit(agents::run_bridge());
    }
    app::run()
}
