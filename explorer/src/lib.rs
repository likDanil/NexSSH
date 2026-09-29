//! "Open with NexSSH" in Windows 11's context menu for folders.
//!
//! The compact menu of Windows 11 only lists commands that packaged apps declare: an
//! `IExplorerCommand` COM server named in a package manifest. NexSSH is not a packaged app, so
//! it registers, for the user, a *package with external location* (a "sparse package", like
//! VS Code's "Open with Code") that holds nothing but that manifest; the COM server itself
//! (`explorer/command`, [`DLL_NAME`]) is unpacked next to it. [`register`] does that.
//!
//! The COM server reads the entry's title and the program to run from [`SETTINGS_KEY`] every
//! time the menu opens; the app keeps them current (the title follows its language), and
//! without them the entry is hidden.
#![cfg(windows)]

pub mod register;

use windows::core::GUID;

/// The COM class of the command (also in `package/AppxManifest.xml`).
pub const CLSID: GUID = GUID::from_u128(0x3ad0b11b_8ce6_4687_a20a_3fa3067c3bb0);

/// The package's name and publisher (`package/AppxManifest.xml`). The OID in the publisher is
/// the one Windows requires of unsigned packages.
pub const PACKAGE_NAME: &str = "NexSSH.ExplorerMenu";
pub const PUBLISHER: &str = "CN=NexSSH, OID.2.25.311729368913984317654407730594956997722=1";

/// The COM server, next to the package (`Path` in the manifest).
pub const DLL_NAME: &str = "nexssh_explorer_command.dll";

/// Under `HKEY_CURRENT_USER`: `Title` of the menu entry and `Command`, the NexSSH.exe it runs
/// (the entry is hidden without them); `Package`, the build the registered package is from.
pub const SETTINGS_KEY: &str = r"Software\NexSSH\ExplorerMenu";

/// Lets any process come to the foreground. A second start of NexSSH (from the menu) hands its
/// folder to the running window, which then comes to the front: Windows allows that only when
/// the process that has the right passes it on.
pub fn allow_foreground() {
    use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
    // SAFETY: a plain Win32 call without pointers.
    let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
}
