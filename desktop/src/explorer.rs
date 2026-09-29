//! "Open with NexSSH" in the context menu of folders in Windows Explorer: on a folder, on the
//! empty space inside one, and on a drive. It opens a local terminal there
//! (`NexSSH --cwd <folder>`, see `local.rs`).
//!
//! * Windows 11's compact menu lists only commands that packaged apps declare. The release build
//!   embeds a package that declares one (`explorer/`, `scripts/explorer-package.ps1`), and it is
//!   registered here, for the user. Explorer shows that command under "Show more options" as
//!   well, so the classic entries for folders go then; drives keep theirs (packages cannot offer
//!   commands on drives). Windows registers the package only once the computer trusts the
//!   certificate it is signed with, which the user allows once with administrator rights
//!   ([`trust`], a button in the settings).
//! * Otherwise (Windows 10, a build without the package, the certificate not trusted yet, or
//!   when registering fails), classic entries under `HKCU\Software\Classes`.
//!
//! The uninstaller removes all of it through `NexSSH --explorer-cleanup` (`windows/hooks.nsh`).

use std::io;
use std::path::Path;
use std::sync::Mutex;

use nexssh_explorer::{register, trust};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

use crate::local::MenuState;

const FOLDER_KEYS: [&str; 2] = [
    r"Software\Classes\Directory\shell\NexSSH",
    r"Software\Classes\Directory\Background\shell\NexSSH",
];
const DRIVE_KEY: &str = r"Software\Classes\Drive\shell\NexSSH";

/// One at a time: the interface asks at start-up, and again when its language changes.
static BUSY: Mutex<()> = Mutex::new(());

/// Adds the entries (again: the label follows the interface's language, the command this copy
/// of NexSSH), or removes them.
pub fn set(enabled: bool) -> io::Result<MenuState> {
    let _busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    if register::runs_in_package() {
        return set_in_package(enabled);
    }
    if !enabled {
        cleanup();
        return Ok(MenuState::default());
    }
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let exe = std::env::current_exe()?;
    let label = nexssh_core::i18n::explorer_menu_label();
    let mut state = MenuState::default();
    if windows_11_menu() {
        match package() {
            Some(payload) if !trust::trusted(payload.cer) => {
                log::info!("the Windows 11 menu entry waits for its certificate to be trusted");
                state.needs_trust = true;
            }
            Some(payload) => {
                register::write_settings(&label, &exe)?;
                match register::install(&payload) {
                    Ok(()) => {
                        remove(&user, &FOLDER_KEYS)?;
                        add(&user, &[DRIVE_KEY], &label, &exe)?;
                        state.modern = true;
                        return Ok(state);
                    }
                    Err(e) => {
                        log::warn!("the Windows 11 menu entry is not registered: {e}");
                        state.error = Some(e);
                    }
                }
            }
            None => log::info!("this build has no package for the Windows 11 menu"),
        }
    }
    // No package left behind, whatever happened before.
    register::uninstall();
    add(
        &user,
        &[FOLDER_KEYS[0], FOLDER_KEYS[1], DRIVE_KEY],
        &label,
        &exe,
    )?;
    Ok(state)
}

/// Puts the entry into Windows 11's compact menu: has the computer trust the package's
/// certificate, which Windows asks the user to allow (UAC), and registers the package. The
/// state is unchanged when the user says no.
pub fn trust() -> io::Result<MenuState> {
    let payload = package().ok_or_else(|| io::Error::other("this build has no package"))?;
    if !trust::trusted(payload.cer) {
        let exe = std::env::current_exe()?;
        match trust::run_elevated(&exe, "--explorer-trust")? {
            None => log::info!("trusting the Explorer menu's certificate was not allowed"),
            Some(0) => log::info!("the computer trusts the Explorer menu's certificate"),
            Some(code) => {
                return Err(io::Error::other(format!(
                    "cannot trust the package's certificate: {}",
                    trust::message(code)
                )));
            }
        }
    }
    set(true)
}

/// For `NexSSH --explorer-trust`, which [`trust`] starts with administrator rights: the
/// computer trusts the package's certificate from now on. Returns the exit code: 0, or the
/// error's code (`trust::message`).
pub fn trust_computer() -> i32 {
    // E_FAIL
    let Some(payload) = package() else {
        return 0x8000_4005_u32 as i32;
    };
    match trust::add(payload.cer) {
        Ok(()) => 0,
        Err(e) => e.code().0,
    }
}

/// For `NexSSH --explorer-untrust`, which [`uninstall`] starts with administrator rights.
pub fn untrust_computer() -> i32 {
    match trust::remove() {
        Ok(()) => 0,
        Err(e) => e.code().0,
    }
}

/// For the uninstaller: removes the entries and the package, and the computer's trust in its
/// certificate, which Windows asks the user to allow.
pub fn uninstall() {
    cleanup();
    if trust::any_trusted()
        && let Ok(exe) = std::env::current_exe()
    {
        let _ = trust::run_elevated(&exe, "--explorer-untrust");
    }
}

/// For a NexSSH that has the package's identity (`register::runs_in_package`): only the settings
/// of the entry change, and the package stays as it is until NexSSH starts without it.
fn set_in_package(enabled: bool) -> io::Result<MenuState> {
    if !enabled {
        // Hides the entry at once.
        register::delete_settings()?;
        remove(
            &RegKey::predef(HKEY_CURRENT_USER),
            &[FOLDER_KEYS[0], FOLDER_KEYS[1], DRIVE_KEY],
        )?;
        return Ok(MenuState::default());
    }
    let label = nexssh_core::i18n::explorer_menu_label();
    register::write_settings(&label, &std::env::current_exe()?)?;
    Ok(MenuState {
        modern: true,
        ..MenuState::default()
    })
}

/// Removes the entries of this user: the classic ones, the package and its files.
pub fn cleanup() {
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let _ = remove(&user, &[FOLDER_KEYS[0], FOLDER_KEYS[1], DRIVE_KEY]);
    register::uninstall();
}

/// Windows 11 (build 22000 and later) with its own context menu, not the classic one brought
/// back by a registry tweak.
fn windows_11_menu() -> bool {
    nexssh_core::local::windows_build().is_some_and(|build| build >= 22000)
        && !register::classic_menu_forced()
}

fn add(user: &RegKey, keys: &[&str], label: &str, exe: &Path) -> io::Result<()> {
    let icon = format!("{},0", exe.display());
    // `%V` is the folder (also for the empty space in a folder); `local.rs` reads a drive's
    // root back from the quoting quirk it comes with.
    let command = format!("\"{}\" --cwd \"%V\"", exe.display());
    for path in keys {
        let (key, _) = user.create_subkey(path)?;
        key.set_value("", &label)?;
        key.set_value("MUIVerb", &label)?;
        key.set_value("Icon", &icon)?;
        let (command_key, _) = key.create_subkey("command")?;
        command_key.set_value("", &command)?;
    }
    Ok(())
}

fn remove(user: &RegKey, keys: &[&str]) -> io::Result<()> {
    for path in keys {
        match user.delete_subkey_all(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// The package this build embeds (`build.rs`).
#[cfg(explorer_package)]
fn package() -> Option<register::Payload<'static>> {
    macro_rules! embedded {
        ($name:literal) => {
            include_bytes!(concat!(env!("NEXSSH_EXPLORER_PACKAGE"), "/", $name))
        };
    }
    Some(register::Payload {
        msix: embedded!("NexSSH.msix"),
        cer: embedded!("NexSSH.cer"),
        dll: embedded!("nexssh_explorer_command.dll"),
        logo: embedded!("logo.png"),
        id: env!("NEXSSH_EXPLORER_PACKAGE_ID"),
    })
}

#[cfg(not(explorer_package))]
fn package() -> Option<register::Payload<'static>> {
    None
}
