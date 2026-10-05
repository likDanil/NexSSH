//! "Open with NexSSH" in the context menu of folders in Windows Explorer: on a folder, on the
//! empty space inside one, and on a drive. It opens a local terminal there
//! (`NexSSH --cwd <folder>`, see `local.rs`).
//!
//! * Windows 11's compact menu lists only commands that packaged apps declare. The release build
//!   embeds a package that declares one (`explorer/`, `scripts/explorer-package.ps1`), and it is
//!   registered here, for the user. Explorer shows that command under "Show more options" as
//!   well, so the classic entries for folders go then; drives keep theirs (packages cannot offer
//!   commands on drives). Windows registers the package only once the computer trusts the
//!   certificate it is signed with, which the user allows once with administrator rights: when
//!   installing NexSSH ([`install`]), or later with a button in the settings ([`trust`]).
//! * Otherwise (Windows 10, a build without the package, the certificate not trusted yet, or
//!   when registering fails), classic entries under `HKCU\Software\Classes`.
//!
//! The installer puts the entries in place right away (`NexSSH --explorer-install`), and the
//! uninstaller removes all of it (`NexSSH --explorer-cleanup`); see `windows/hooks.nsh`.

use std::io;
use std::path::Path;
use std::sync::Mutex;

use nexssh_core::i18n::{self, Lang};
use nexssh_explorer::{register, trust};
use serde_json::Value;
use windows::Win32::Foundation::HWND;
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

use crate::local::MenuState;
use crate::settings::Settings;

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
    // A development build has no package: it would take the installed NexSSH's away and point
    // the entries at itself. It leaves them alone, unless asked (to work on this very feature).
    if cfg!(debug_assertions) && std::env::var_os("NEXSSH_EXPLORER_MENU").is_none() {
        log::info!("a development build leaves the Explorer menu alone (NEXSSH_EXPLORER_MENU=1)");
        return Ok(MenuState::default());
    }
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
    ask_trust(None)?;
    set(true)
}

/// `NexSSH --explorer-install [--trust] [--window <handle>]`, which the installer runs once the
/// files are in place: see [`install`]. Exits with 0, or 1 after an error; the installer's
/// details list what went wrong.
pub fn install_command(args: &[String]) -> i32 {
    match install(args.iter().any(|a| a == "--trust"), window(args)) {
        Ok(Some(state)) if state.modern => {
            log::info!("the Explorer menu entry is in Windows 11's compact menu")
        }
        Ok(Some(_)) => log::info!("the Explorer menu entry is a classic one"),
        Ok(None) => log::info!("the Explorer menu entry is turned off in the settings"),
        Err(e) => {
            log::error!("cannot set up the Explorer menu entry: {e}");
            return 1;
        }
    }
    0
}

/// `NexSSH --explorer-cleanup [--keep-trust] [--window <handle>]`, which the uninstaller runs:
/// see [`uninstall`].
pub fn uninstall_command(args: &[String]) {
    uninstall(args.iter().any(|a| a == "--keep-trust"), window(args));
}

/// Puts the entries in place for the user who installs NexSSH, as the page would on its first
/// start, unless they turned them off in the settings (`None` then). With `ask` (an install the
/// user watches), Windows 11's compact menu does not wait for the button in the settings:
/// Windows asks the user to let the computer trust the package's certificate, if it does not
/// yet. The question belongs to the `owner` window, the installer's. Without `ask` (updates,
/// silent installs), the entries are what the computer allows already.
fn install(ask: bool, owner: Option<HWND>) -> io::Result<Option<MenuState>> {
    let settings = nexssh_core::default_data_dir().map(|dir| Settings::load(&dir));
    let setting = |name: &str| settings.as_ref().and_then(|s| s.value().get(name).cloned());
    if setting("explorerMenu").and_then(|v| v.as_bool()) == Some(false) {
        return Ok(None);
    }
    // The label in the language the page shows (it tells the backend when it starts).
    let language = setting("language");
    let language = interface_language(language.as_ref().and_then(Value::as_str));
    i18n::set_language(language);
    let state = set(true)?;
    if !(ask && state.needs_trust) {
        return Ok(Some(state));
    }
    ask_trust(owner)?;
    set(true).map(Some)
}

/// Has Windows ask the user to let the computer trust the package's certificate (UAC), unless
/// it does already; `owner`: the window the question belongs to. Nothing changes when the user
/// says no.
fn ask_trust(owner: Option<HWND>) -> io::Result<()> {
    let payload = package().ok_or_else(|| io::Error::other("this build has no package"))?;
    if trust::trusted(payload.cer) {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    match trust::run_elevated(&exe, "--explorer-trust", owner)? {
        None => log::info!("trusting the Explorer menu's certificate was not allowed"),
        Some(0) => log::info!("the computer trusts the Explorer menu's certificate"),
        Some(code) => {
            return Err(io::Error::other(format!(
                "cannot trust the package's certificate: {}",
                trust::message(code)
            )));
        }
    }
    Ok(())
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

/// For the uninstaller: removes the entries and the package, and, unless `keep_trust`, the
/// computer's trust in its certificate, which Windows asks the user to allow (the question
/// belongs to the `owner` window, the uninstaller's). The trust stays when the installer of
/// another version runs the uninstaller first: that installer puts the entries back right
/// after, and the user is not asked twice.
fn uninstall(keep_trust: bool, owner: Option<HWND>) {
    cleanup();
    if !keep_trust
        && trust::any_trusted()
        && let Ok(exe) = std::env::current_exe()
    {
        let _ = trust::run_elevated(&exe, "--explorer-untrust", owner);
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

/// `--window <handle>`: a window of the installer or the uninstaller (NSIS's `$HWNDPARENT`, a
/// number; 0 when it runs silently).
fn window(args: &[String]) -> Option<HWND> {
    let at = args.iter().position(|a| a == "--window")?;
    let handle: isize = args.get(at + 1)?.parse().ok()?;
    (handle != 0).then_some(HWND(handle as *mut std::ffi::c_void))
}

/// The interface's language, chosen the way the page chooses it: the one in the settings, else
/// the first of the user's display languages in Windows that NexSSH has, else English.
fn interface_language(setting: Option<&str>) -> Lang {
    if let Some(lang) = setting.and_then(Lang::from_tag) {
        return lang;
    }
    let system = windows_languages();
    system
        .iter()
        .find_map(|tag| Lang::from_tag(tag))
        .unwrap_or_default()
}

/// The user's display languages in Windows, the preferred first (`ru-RU`, `en-US`…).
fn windows_languages() -> Vec<String> {
    use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    use windows::core::PWSTR;

    let (mut count, mut len) = (0, 0);
    // SAFETY: the first call asks for the length, the second fills a buffer of that length.
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut len).is_err() {
            return Vec::new();
        }
        let mut buffer = vec![0u16; len as usize];
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut len,
        )
        .is_err()
        {
            return Vec::new();
        }
        // Each name ends with a null, the list with another one.
        buffer
            .split(|&c| c == 0)
            .filter(|name| !name.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }
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
