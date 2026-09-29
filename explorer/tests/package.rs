//! Registers the package that `scripts/explorer-package.ps1` built and asks its COM server for
//! the menu entry, the way Explorer does. Runs where `NEXSSH_EXPLORER_PACKAGE` names the
//! script's output folder (the Windows CI job); passes trivially elsewhere.
//!
//! NexSSH registers the package without administrator rights (under UAC, an administrator's
//! programs run without them as well), so the test does too: started with them, as on CI, it
//! runs itself again with a token like the ones UAC gives, and that run has to pass.
#![cfg(windows)]

use std::ffi::c_void;
use std::path::PathBuf;

use nexssh_explorer::register::{self, Payload};
use nexssh_explorer::{CLSID, DLL_NAME};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    CheckTokenMembership, CreateRestrictedToken, CreateWellKnownSid, DISABLE_MAX_PRIVILEGE,
    LUA_TOKEN, PSID, SECURITY_MAX_SID_SIZE, SID_AND_ATTRIBUTES, SetTokenInformation,
    TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel, WELL_KNOWN_SID_TYPE, WinBuiltinAdministratorsSid,
    WinMediumLabelSid,
};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows::Win32::System::SystemServices::SE_GROUP_INTEGRITY;
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcessToken,
    PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW,
    WaitForSingleObject,
};
use windows::Win32::UI::Shell::{ECS_ENABLED, ECS_HIDDEN, IExplorerCommand, IShellItemArray};
use windows::core::{BOOL, PCWSTR, PWSTR};

const TEST: &str = "registers_the_package_and_its_com_server_serves_the_entry";

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
    if administrator() {
        assert_eq!(
            run_without_administrator_rights(),
            0,
            "the run without administrator rights failed (its output is above)"
        );
        assert!(!register::installed());
        return;
    }
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let (msix, dll, logo) = (read("NexSSH.msix"), read(DLL_NAME), read("logo.png"));
    let payload = Payload {
        msix: &msix,
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

/// Whether this process has administrator rights.
fn administrator() -> bool {
    let admins = well_known_sid(WinBuiltinAdministratorsSid);
    let mut member = BOOL(0);
    // SAFETY: the SID outlives the call.
    unsafe { CheckTokenMembership(None, PSID(admins.as_ptr().cast_mut().cast()), &mut member) }
        .is_ok()
        && member.as_bool()
}

fn well_known_sid(kind: WELL_KNOWN_SID_TYPE) -> Vec<u8> {
    let mut sid = vec![0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut size = sid.len() as u32;
    // SAFETY: the buffer holds `size` bytes.
    unsafe { CreateWellKnownSid(kind, None, Some(PSID(sid.as_mut_ptr().cast())), &mut size) }
        .unwrap();
    sid.truncate(size as usize);
    sid
}

/// Runs this test again the way UAC runs an administrator's programs: the Administrators group
/// only denies access, the privileges are the basic ones, the integrity level is medium. It
/// writes to this process's output; returns its exit code.
fn run_without_administrator_rights() -> u32 {
    let admins = well_known_sid(WinBuiltinAdministratorsSid);
    let medium = well_known_sid(WinMediumLabelSid);
    let exe = std::env::current_exe().unwrap();
    let mut command: Vec<u16> = format!("\"{}\" {TEST} --exact --nocapture", exe.display())
        .encode_utf16()
        .chain([0])
        .collect();
    // SAFETY: the SIDs, the label and the command line outlive the calls that use them; every
    // handle is closed.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
            &mut token,
        )
        .unwrap();
        let disable = [SID_AND_ATTRIBUTES {
            Sid: PSID(admins.as_ptr().cast_mut().cast()),
            Attributes: 0,
        }];
        let mut restricted = HANDLE::default();
        CreateRestrictedToken(
            token,
            DISABLE_MAX_PRIVILEGE | LUA_TOKEN,
            Some(&disable),
            None,
            None,
            &mut restricted,
        )
        .unwrap();
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: PSID(medium.as_ptr().cast_mut().cast()),
                Attributes: SE_GROUP_INTEGRITY as u32,
            },
        };
        SetTokenInformation(
            restricted,
            TokenIntegrityLevel,
            (&raw const label).cast::<c_void>(),
            (size_of::<TOKEN_MANDATORY_LABEL>() + medium.len()) as u32,
        )
        .unwrap();

        let startup = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            hStdInput: GetStdHandle(STD_INPUT_HANDLE).unwrap_or_default(),
            hStdOutput: GetStdHandle(STD_OUTPUT_HANDLE).unwrap_or_default(),
            hStdError: GetStdHandle(STD_ERROR_HANDLE).unwrap_or_default(),
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        CreateProcessAsUserW(
            Some(restricted),
            PCWSTR::null(),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            true,
            PROCESS_CREATION_FLAGS(0),
            None,
            PCWSTR::null(),
            &startup,
            &mut process,
        )
        .unwrap_or_else(|e| panic!("cannot start the test without administrator rights: {e}"));
        WaitForSingleObject(process.hProcess, INFINITE);
        let mut code = 1;
        GetExitCodeProcess(process.hProcess, &mut code).unwrap();
        for handle in [process.hProcess, process.hThread, restricted, token] {
            let _ = CloseHandle(handle);
        }
        code
    }
}
