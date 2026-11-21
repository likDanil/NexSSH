#[path = "../../explorer/menu.rs"]
mod shared;
pub use shared::{
    MenuState, install_command, set, trust, trust_computer, uninstall_command, untrust_computer,
};
fn load_preferences() -> Option<serde_json::Value> {
    nexssh_core::default_data_dir().map(|dir| crate::settings::Settings::load(&dir).value().clone())
}
