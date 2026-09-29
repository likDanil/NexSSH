//! "Open with NexSSH" in the context menu of folders in Windows Explorer: on a folder, on
//! the empty space inside one, and on a drive. It opens a local terminal there
//! (`NexSSH --cwd <folder>`, see `local.rs`).
//!
//! Classic menu entries for the current user (HKCU, no administrator rights). Windows 11
//! lists them under "Show more options": its compact menu only takes packaged apps. The
//! uninstaller removes the keys too (`windows/hooks.nsh`).

use std::io;

use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

const KEYS: [&str; 3] = [
    r"Software\Classes\Directory\shell\NexSSH",
    r"Software\Classes\Directory\Background\shell\NexSSH",
    r"Software\Classes\Drive\shell\NexSSH",
];

/// Adds the entries (again: the label follows the interface's language, the command this
/// copy of NexSSH), or removes them.
pub fn set(enabled: bool) -> io::Result<()> {
    let user = RegKey::predef(HKEY_CURRENT_USER);
    if !enabled {
        for path in KEYS {
            match user.delete_subkey_all(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let label = nexssh_core::i18n::explorer_menu_label();
    let icon = format!("{},0", exe.display());
    // `%V` is the folder (for the empty space in a folder too).
    let command = format!("\"{}\" --cwd \"%V\"", exe.display());
    for path in KEYS {
        let (key, _) = user.create_subkey(path)?;
        key.set_value("", &label)?;
        key.set_value("MUIVerb", &label)?;
        key.set_value("Icon", &icon)?;
        let (command_key, _) = key.create_subkey("command")?;
        command_key.set_value("", &command)?;
    }
    Ok(())
}
