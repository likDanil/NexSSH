#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod prompts;
mod terminal;

slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}
