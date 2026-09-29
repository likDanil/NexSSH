//! Registers the package that `scripts/explorer-package.ps1` built and asks its COM server for
//! the menu entry, the way Explorer does. Runs where `NEXSSH_EXPLORER_PACKAGE` names the
//! script's output folder (the Windows CI job); passes trivially elsewhere.
#![cfg(windows)]

use std::path::PathBuf;

use nexssh_explorer::register::{self, Payload};
use nexssh_explorer::{CLSID, DLL_NAME};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::UI::Shell::{ECS_ENABLED, ECS_HIDDEN, IExplorerCommand, IShellItemArray};

/// Removes what the test registered, also when it fails.
struct Registered;

impl Drop for Registered {
    fn drop(&mut self) {
        register::uninstall();
    }
}

#[test]
fn registers_the_package_and_its_com_server_serves_the_entry() {
    let Some(dir) = std::env::var_os("NEXSSH_EXPLORER_PACKAGE").map(PathBuf::from) else {
        return;
    };
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let (msix, cer, dll, logo) = (
        read("NexSSH.msix"),
        read("NexSSH.cer"),
        read(DLL_NAME),
        read("logo.png"),
    );
    let payload = Payload {
        msix: &msix,
        cer: &cer,
        dll: &dll,
        logo: &logo,
        id: "ci-test",
    };

    // Any existing program: the entry is asked for, not run.
    let program = std::env::current_exe().unwrap();
    let registered = Registered;
    register::write_settings("Open with NexSSH (test)", &program).unwrap();
    register::install(&payload).unwrap_or_else(|e| panic!("{e}"));
    assert!(register::installed());
    // Registered already: nothing to do the second time.
    register::install(&payload).unwrap_or_else(|e| panic!("{e}"));
    // This process was not started by the package.
    assert!(!register::runs_in_package());

    // Asked the way Explorer asks, from a thread of its own (an STA, as Explorer's are).
    std::thread::spawn(move || {
        // SAFETY: COM calls with valid arguments; the strings are freed once read.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            let command: IExplorerCommand = CoCreateInstance(&CLSID, None, CLSCTX_LOCAL_SERVER)
                .unwrap_or_else(|e| panic!("the package's COM server: {e}"));
            let title = command.GetTitle(None::<&IShellItemArray>).unwrap();
            assert_eq!(title.to_string().unwrap(), "Open with NexSSH (test)");
            CoTaskMemFree(Some(title.0.cast_const().cast()));
            let icon = command.GetIcon(None::<&IShellItemArray>).unwrap();
            assert_eq!(icon.to_string().unwrap(), program.to_string_lossy());
            CoTaskMemFree(Some(icon.0.cast_const().cast()));
            assert_eq!(
                command.GetState(None::<&IShellItemArray>, false).unwrap(),
                ECS_ENABLED.0 as u32
            );

            // Turned off: the entry hides at once.
            register::delete_settings().unwrap();
            assert_eq!(
                command.GetState(None::<&IShellItemArray>, false).unwrap(),
                ECS_HIDDEN.0 as u32
            );
        }
    })
    .join()
    .unwrap();

    drop(registered);
    assert!(!register::installed());
}
