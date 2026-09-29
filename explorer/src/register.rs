//! Registers the menu entry's package for the current user, and removes it.
//!
//! No administrator rights are needed, and no certificate: the package is registered for the
//! user, unsigned. Windows 11 allows that for a package that holds no program code (this one
//! holds its manifest and logo; the COM server is outside it, in the external location), and
//! the publisher's OID ([`PUBLISHER`]) marks it as unsigned, so it can never pass for a signed
//! package.

use std::io;
use std::path::{Path, PathBuf};

use windows::ApplicationModel::Package;
use windows::Foundation::Uri;
use windows::Management::Deployment::{AddPackageOptions, DeploymentResult, PackageManager};
use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
use windows::core::{HSTRING, PWSTR};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

use crate::{DLL_NAME, PACKAGE_NAME, PUBLISHER, SETTINGS_KEY};

/// The package's file, next to the COM server.
const MSIX_NAME: &str = "NexSSH.msix";
/// The manifest's logo (`package/logo.png`).
const LOGO_NAME: &str = "logo.png";

/// What a build of NexSSH brings along; `scripts/explorer-package.ps1` makes the files.
pub struct Payload<'a> {
    /// The package: the manifest and the logo.
    pub msix: &'a [u8],
    /// The COM server.
    pub dll: &'a [u8],
    pub logo: &'a [u8],
    /// Tells builds apart: their files are unpacked into a folder of this name, so a COM server
    /// that Explorer still has loaded never has to be replaced.
    pub id: &'a str,
}

/// Errors are messages for the log.
pub type Result<T> = std::result::Result<T, String>;

/// Sets the title of the menu entry and the program it runs.
pub fn write_settings(title: &str, program: &Path) -> io::Result<()> {
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(SETTINGS_KEY)?;
    key.set_value("Title", &title)?;
    key.set_value("Command", &program.as_os_str())
}

/// Hides the menu entry (the COM server shows it only with its settings) and forgets which
/// build is registered.
pub fn delete_settings() -> io::Result<()> {
    match RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(SETTINGS_KEY) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Registers the package of `payload` for the current user, unless it already is: unpacks the
/// files and replaces the package of another build. Quick when nothing changed, so it can run
/// at every start.
pub fn install(payload: &Payload) -> Result<()> {
    let base = base_dir().ok_or("the LOCALAPPDATA folder is unknown")?;
    let dir = base.join(payload.id);
    unpack(&dir, payload).map_err(|e| format!("cannot unpack into {}: {e}", dir.display()))?;
    let manager = PackageManager::new().map_err(|e| e.message())?;
    let registered = find(&manager)?;
    if !registered.is_empty() && registered_build().as_deref() == Some(payload.id) {
        remove_other_builds(&base, Some(payload.id));
        return Ok(());
    }
    for package in &registered {
        remove_package(&manager, package)?;
    }
    add_package(&manager, &dir)?;
    set_registered_build(payload.id).map_err(|e| e.to_string())?;
    // The files of earlier builds are not needed any more.
    remove_other_builds(&base, Some(payload.id));
    Ok(())
}

/// Whether the package is registered for the current user.
pub fn installed() -> bool {
    PackageManager::new()
        .ok()
        .and_then(|manager| find(&manager).ok())
        .is_some_and(|packages| !packages.is_empty())
}

/// Removes everything [`install`] and [`write_settings`] put in place: the package, the
/// settings and the unpacked files (a COM server that Explorer still has loaded stays until the
/// next time).
pub fn uninstall() {
    if let Ok(manager) = PackageManager::new()
        && let Ok(registered) = find(&manager)
    {
        for package in &registered {
            let _ = remove_package(&manager, package);
        }
    }
    let _ = delete_settings();
    if let Some(base) = base_dir() {
        remove_other_builds(&base, None);
        let _ = std::fs::remove_dir(&base);
    }
}

/// The user brought Windows 10's context menu back (the well-known registry tweak). VS Code
/// uses its classic entries then, and so does NexSSH.
pub fn classic_menu_forced() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(
            r"Software\Classes\CLSID\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}\InprocServer32",
        )
        .is_ok()
}

/// Whether this process has the package's identity: NexSSH started by the package's COM server
/// itself (when Explorer could not start it), or by such a process. The package cannot be
/// removed or replaced under it: Windows would end the process.
pub fn runs_in_package() -> bool {
    let mut len = 0;
    // SAFETY: the first call asks for the length, the second fills a buffer of that length.
    unsafe {
        if GetCurrentPackageFullName(&mut len, None) != ERROR_INSUFFICIENT_BUFFER {
            return false;
        }
        let mut name = vec![0u16; len as usize];
        if GetCurrentPackageFullName(&mut len, Some(PWSTR(name.as_mut_ptr()))).is_err() {
            return false;
        }
        // "NexSSH.ExplorerMenu_0.3.0.0_x64__…"; the length includes the terminating null.
        String::from_utf16_lossy(&name[..(len as usize).saturating_sub(1).min(name.len())])
            .starts_with(&format!("{PACKAGE_NAME}_"))
    }
}

/// `%LOCALAPPDATA%\NexSSH\explorer`: a folder per build.
fn base_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(|dir| PathBuf::from(dir).join(r"NexSSH\explorer"))
}

/// Writes the build's files unless they are there already (a build's folder never changes).
fn unpack(dir: &Path, payload: &Payload) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let files = [
        (DLL_NAME, payload.dll),
        (MSIX_NAME, payload.msix),
        (LOGO_NAME, payload.logo),
    ];
    for (name, data) in files {
        let path = dir.join(name);
        if !path.metadata().is_ok_and(|m| m.len() == data.len() as u64) {
            std::fs::write(&path, data)?;
        }
    }
    Ok(())
}

/// Deletes the folders of other builds (all of them without `keep`). A COM server that
/// Explorer has loaded cannot be deleted; its folder goes the next time.
fn remove_other_builds(base: &Path, keep: Option<&str>) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        if keep.is_none_or(|keep| entry.file_name() != keep) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn registered_build() -> Option<String> {
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(SETTINGS_KEY)
        .ok()?;
    key.get_value("Package").ok()
}

fn set_registered_build(id: &str) -> io::Result<()> {
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(SETTINGS_KEY)?;
    key.set_value("Package", &id)
}

fn find(manager: &PackageManager) -> Result<Vec<Package>> {
    // An empty user id means the current user.
    let packages = manager
        .FindPackagesByUserSecurityIdNamePublisher(
            &HSTRING::new(),
            &HSTRING::from(PACKAGE_NAME),
            &HSTRING::from(PUBLISHER),
        )
        .map_err(|e| e.message())?;
    Ok(packages.into_iter().collect())
}

fn add_package(manager: &PackageManager, dir: &Path) -> Result<()> {
    let options = AddPackageOptions::new().map_err(|e| e.message())?;
    options
        .SetExternalLocationUri(&uri(dir)?)
        .map_err(|e| e.message())?;
    // Unsigned (see the top of this file).
    options.SetAllowUnsigned(true).map_err(|e| e.message())?;
    let operation = manager
        .AddPackageByUriAsync(&uri(&dir.join(MSIX_NAME))?, &options)
        .map_err(|e| e.message())?;
    finished(operation.join(), "cannot register the package")
}

fn remove_package(manager: &PackageManager, package: &Package) -> Result<()> {
    let name = package
        .Id()
        .and_then(|id| id.FullName())
        .map_err(|e| e.message())?;
    let operation = manager.RemovePackageAsync(&name).map_err(|e| e.message())?;
    finished(operation.join(), "cannot remove the package")
}

/// A deployment operation's outcome. A failed one still has a result, with the reason.
fn finished(result: windows::core::Result<DeploymentResult>, what: &str) -> Result<()> {
    match result {
        Ok(result) => match result.ExtendedErrorCode() {
            Ok(code) if code.is_err() => {
                let text = result
                    .ErrorText()
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                Err(format!("{what}: {} ({code})", text.trim()))
            }
            _ => Ok(()),
        },
        Err(e) => Err(format!("{what}: {}", e.message())),
    }
}

/// `file:///C:/Users/…` for `path`: everything but ASCII letters, digits and `-._~/:`
/// percent-encoded (UTF-8), as `Uri` expects.
fn file_url(path: &Path) -> String {
    let mut url = String::from("file:///");
    for byte in path.to_string_lossy().replace('\\', "/").bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            url.push(byte as char);
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

fn uri(path: &Path) -> Result<Uri> {
    Uri::CreateUri(&HSTRING::from(file_url(path))).map_err(|e| e.message())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_escape_what_uris_cannot_hold() {
        assert_eq!(
            file_url(Path::new(
                r"C:\Users\me\AppData\Local\NexSSH\explorer\0.3.0-ab12"
            )),
            "file:///C:/Users/me/AppData/Local/NexSSH/explorer/0.3.0-ab12"
        );
        assert_eq!(
            file_url(Path::new(r"C:\Users\Иван Петров\x#1%.msix")),
            "file:///C:/Users/%D0%98%D0%B2%D0%B0%D0%BD%20%D0%9F%D0%B5%D1%82%D1%80%D0%BE%D0%B2/x%231%25.msix"
        );
        assert!(uri(Path::new(r"C:\Users\Иван Петров\NexSSH.msix")).is_ok());
    }
}
