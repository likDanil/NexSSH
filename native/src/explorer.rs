//! A separate per-user Explorer entry for the experimental client.
#[cfg(windows)]
pub fn apply(enabled: bool) -> std::io::Result<()> {
    if cfg!(debug_assertions) && std::env::var_os("NEXSSH_EXPLORER_MENU").is_none() {
        return Ok(());
    }
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    let root = RegKey::predef(HKEY_CURRENT_USER);
    let exe = std::env::current_exe()?;
    for (base, target) in [
        (r"Software\Classes\Directory\shell\NexSSH2", "%1"),
        (r"Software\Classes\Directory\Background\shell\NexSSH2", "%V"),
        (r"Software\Classes\Drive\shell\NexSSH2", "%1"),
    ] {
        if enabled {
            let (key, _) = root.create_subkey(base)?;
            key.set_value("", &"Open with NexSSH 2.0")?;
            key.set_value("Icon", &format!("\"{}\",0", exe.display()))?;
            key.create_subkey("command")?
                .0
                .set_value("", &format!("\"{}\" --cwd \"{target}\"", exe.display()))?;
        } else {
            match root.delete_subkey_all(base) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}
#[cfg(not(windows))]
pub fn apply(_: bool) -> std::io::Result<()> {
    Ok(())
}
