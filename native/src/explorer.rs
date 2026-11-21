//! The installed Slint client uses the same Explorer identity and hooks as 1.x.
#[cfg(windows)]
#[path = "../../explorer/menu.rs"]
mod shared;
#[cfg(windows)]
fn load_preferences() -> Option<serde_json::Value> {
    let dir = nexssh_core::default_data_dir()?;
    crate::settings::Settings::load(&dir).ok().map(|s| s.value)
}
#[cfg(windows)]
pub fn command(args: &[String]) -> Option<i32> {
    match args.first().map(String::as_str)? {
        "--explorer-install" => Some(shared::install_command(args)),
        "--explorer-cleanup" => {
            shared::uninstall_command(args);
            Some(0)
        }
        "--explorer-trust" => Some(shared::trust_computer()),
        "--explorer-untrust" => Some(shared::untrust_computer()),
        _ => None,
    }
}
#[cfg(windows)]
pub fn apply(enabled: bool) -> std::io::Result<()> {
    if std::env::var_os("NEXSSH_DATA_DIR").is_some()
        && std::env::var_os("NEXSSH_EXPLORER_MENU").is_none()
    {
        return Ok(());
    }
    shared::set(enabled).map(|_| ())
}
#[cfg(not(windows))]
pub fn apply(_: bool) -> std::io::Result<()> {
    Ok(())
}
