//! The COM server of "Open with NexSSH" in Windows 11's context menu for folders: an
//! `IExplorerCommand` that Explorer loads, in a COM surrogate, from the package
//! `nexssh-explorer` registers. It works like VS Code's "Open with Code".
//!
//! The entry's title and the program it runs come from the registry
//! ([`nexssh_explorer::SETTINGS_KEY`]) every time the menu opens, so the app can change them
//! (the title follows its language); without them the entry is hidden. On a folder, or on the
//! empty space inside one, it has Explorer run `NexSSH.exe --cwd <folder>`.
#![cfg(windows)]

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use nexssh_explorer::{CLSID, SETTINGS_KEY};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_NOTIMPL, E_POINTER, S_FALSE, S_OK,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, IBindCtx, IClassFactory, IClassFactory_Impl,
    IDispatch, IServiceProvider,
};
use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VT_I4, VariantClear};
use windows::Win32::UI::Shell::{
    CSIDL_DESKTOP, ECF_DEFAULT, ECS_ENABLED, ECS_HIDDEN, IEnumExplorerCommand, IExplorerCommand,
    IExplorerCommand_Impl, IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellItemArray,
    IShellWindows, SHStrDupW, SID_STopLevelBrowser, SIGDN_FILESYSPATH, SVGIO_BACKGROUND,
    SWC_DESKTOP, SWFO_NEEDDISPATCH, ShellExecuteW, ShellWindows,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{
    BOOL, BSTR, GUID, HRESULT, HSTRING, IUnknown, Interface, PCWSTR, PWSTR, Ref, Result, w,
};
use windows_core::implement;
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

/// Live commands and `LockServer` locks: the surrogate may unload the DLL when both are zero.
static OBJECTS: AtomicUsize = AtomicUsize::new(0);
static LOCKS: AtomicUsize = AtomicUsize::new(0);

const DEFAULT_TITLE: &str = "Open with NexSSH";

#[implement(IExplorerCommand)]
struct OpenCommand;

impl OpenCommand {
    fn new() -> Self {
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        OpenCommand
    }
}

impl Drop for OpenCommand {
    fn drop(&mut self) {
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

impl IExplorerCommand_Impl for OpenCommand_Impl {
    fn GetTitle(&self, _items: Ref<IShellItemArray>) -> Result<PWSTR> {
        let title = settings().map_or_else(|| DEFAULT_TITLE.to_string(), |s| s.title);
        // SAFETY: the string is copied into memory the caller frees.
        unsafe { SHStrDupW(&HSTRING::from(title)) }
    }

    fn GetIcon(&self, _items: Ref<IShellItemArray>) -> Result<PWSTR> {
        // NexSSH's own icon.
        let settings = settings().ok_or(E_FAIL)?;
        // SAFETY: as above.
        unsafe { SHStrDupW(&HSTRING::from(settings.program.as_os_str())) }
    }

    fn GetToolTip(&self, _items: Ref<IShellItemArray>) -> Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }

    fn GetCanonicalName(&self) -> Result<GUID> {
        Ok(CLSID)
    }

    fn GetState(&self, _items: Ref<IShellItemArray>, _slow_ok: BOOL) -> Result<u32> {
        let state = if settings().is_some() {
            ECS_ENABLED
        } else {
            ECS_HIDDEN
        };
        Ok(state.0 as u32)
    }

    fn Invoke(&self, items: Ref<IShellItemArray>, _bind: Ref<IBindCtx>) -> Result<()> {
        let Some(settings) = settings() else {
            return Ok(());
        };
        // The folder clicked; for the empty space inside a folder, that folder.
        let folders = folders(items.as_ref());
        if folders.is_empty() {
            return Ok(());
        }
        launch(&settings.program, &folders)
    }

    fn GetFlags(&self) -> Result<u32> {
        Ok(ECF_DEFAULT.0 as u32)
    }

    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        Err(E_NOTIMPL.into())
    }
}

struct Settings {
    title: String,
    program: PathBuf,
}

/// The entry's settings, written by NexSSH; `None` hides the entry (turned off, or NexSSH is
/// not where it was).
fn settings() -> Option<Settings> {
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(SETTINGS_KEY)
        .ok()?;
    let program = PathBuf::from(key.get_value::<String, _>("Command").ok()?);
    if !program.is_file() {
        return None;
    }
    let title = key
        .get_value::<String, _>("Title")
        .ok()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_TITLE.to_string());
    Some(Settings { title, program })
}

/// The file system paths of the selected items.
fn folders(items: Option<&IShellItemArray>) -> Vec<String> {
    let Some(items) = items else {
        return Vec::new();
    };
    // SAFETY: `items` is a live array; each display name is freed after it is copied.
    unsafe {
        let count = items.GetCount().unwrap_or(0);
        (0..count)
            .filter_map(|i| {
                let name = items
                    .GetItemAt(i)
                    .ok()?
                    .GetDisplayName(SIGDN_FILESYSPATH)
                    .ok()?;
                let path = name.to_string().ok();
                CoTaskMemFree(Some(name.0.cast_const().cast()));
                path
            })
            .collect()
    }
}

/// Runs NexSSH with a `--cwd` for each folder.
///
/// Explorer starts it, as it starts the programs of its own menus. A process started from here
/// would inherit the package's identity and run in its container, and so would the shells of its
/// terminals: shares like `\\wsl.localhost` are out of reach there, and Windows would end it
/// when the package is replaced (VS Code's "Open with Code" has that problem). Only when
/// Explorer cannot is NexSSH started from here.
fn launch(program: &Path, folders: &[String]) -> Result<()> {
    let args: Vec<String> = folders
        .iter()
        .map(|f| format!("--cwd {}", quote(f)))
        .collect();
    let args = args.join(" ");
    let program = HSTRING::from(program.as_os_str());
    // SAFETY: COM calls Invoke on a thread it initialized.
    if unsafe { explorer_execute(&program, &args, SW_SHOWNORMAL.0) }.is_ok() {
        return Ok(());
    }
    // The new NexSSH may hand the folders to the one already running, which then comes forward.
    nexssh_explorer::allow_foreground();
    // SAFETY: all strings are null-terminated and outlive the call.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &program,
            &HSTRING::from(args),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values up to 32 are errors.
    if result.0 as usize <= 32 {
        return Err(E_FAIL.into());
    }
    Ok(())
}

/// `ShellExecute` in Explorer's process, through the automation object of the desktop's view
/// (Raymond Chen, "How can I launch an unelevated process from my elevated process and vice
/// versa?").
///
/// # Safety
/// COM is initialized on the calling thread.
unsafe fn explorer_execute(program: &HSTRING, args: &str, show: i32) -> Result<()> {
    // SAFETY: COM calls with valid arguments, per the contract.
    unsafe {
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL)?;
        let empty = VARIANT::default();
        let mut hwnd = 0;
        let desktop = windows.FindWindowSW(
            &number(CSIDL_DESKTOP as i32),
            &empty,
            SWC_DESKTOP,
            &mut hwnd,
            SWFO_NEEDDISPATCH,
        )?;
        let browser: IShellBrowser = desktop
            .cast::<IServiceProvider>()?
            .QueryService(&SID_STopLevelBrowser)?;
        let view: IDispatch = browser
            .QueryActiveShellView()?
            .GetItemObject(SVGIO_BACKGROUND)?;
        let shell: IShellDispatch2 = view.cast::<IShellFolderViewDual>()?.Application()?.cast()?;
        shell.ShellExecute(
            &BSTR::from_wide(program),
            &Text::new(args).0,
            &empty,
            &Text::new("open").0,
            &number(show),
        )
    }
}

fn number(value: i32) -> VARIANT {
    let mut variant = VARIANT::default();
    // SAFETY: fills in a zeroed VARIANT.
    unsafe {
        let inner = &mut *variant.Anonymous.Anonymous;
        inner.vt = VT_I4;
        inner.Anonymous.lVal = value;
    }
    variant
}

/// A `VARIANT` with a string, which it frees.
struct Text(VARIANT);

impl Text {
    fn new(value: &str) -> Self {
        let mut variant = VARIANT::default();
        // SAFETY: fills in a zeroed VARIANT.
        unsafe {
            let inner = &mut *variant.Anonymous.Anonymous;
            inner.vt = VT_BSTR;
            inner.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(value));
        }
        Text(variant)
    }
}

impl Drop for Text {
    fn drop(&mut self) {
        // SAFETY: the VARIANT holds a string it owns.
        let _ = unsafe { VariantClear(&mut self.0) };
    }
}

/// Quotes a command line argument the way `CommandLineToArgvW` reads it back: backslashes are
/// doubled only before a quote (or the closing one), so `C:\` becomes `"C:\\"`.
fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"', '\\']) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> Result<()> {
        if object.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: COM passes a writable pointer and a valid interface id.
        unsafe {
            *object = std::ptr::null_mut();
            if !outer.is_null() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let command: IUnknown = OpenCommand::new().into();
            command.query(iid, object).ok()
        }
    }

    fn LockServer(&self, lock: BOOL) -> Result<()> {
        if lock.as_bool() {
            LOCKS.fetch_add(1, Ordering::SeqCst);
        } else {
            LOCKS.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// Hands COM the class factory of the command.
///
/// # Safety
/// COM calls it with valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    if object.is_null() {
        return E_POINTER;
    }
    // SAFETY: per the contract.
    unsafe {
        *object = std::ptr::null_mut();
        if clsid.is_null() || *clsid != CLSID {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = Factory.into();
        factory.query(iid, object)
    }
}

/// Whether COM may unload the DLL: no command is alive and nothing locks the server.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if OBJECTS.load(Ordering::SeqCst) == 0 && LOCKS.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::Shell::{
        CommandLineToArgvW, IShellItem, SHCreateItemFromParsingName,
        SHCreateShellItemArrayFromShellItem,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn com() {
        // SAFETY: initializes COM for the test's thread.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    }

    fn items(path: &str) -> IShellItemArray {
        com();
        // SAFETY: plain shell calls with a null-terminated path.
        unsafe {
            let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None).unwrap();
            SHCreateShellItemArrayFromShellItem(&item).unwrap()
        }
    }

    /// Splits a command line the way the started program will.
    fn split(line: &str) -> Vec<String> {
        let mut count = 0;
        // SAFETY: the returned array is read and then freed with LocalFree's replacement below.
        unsafe {
            let argv = CommandLineToArgvW(&HSTRING::from(line), &mut count);
            let args = (0..count as usize)
                .map(|i| (*argv.add(i)).to_string().unwrap())
                .collect();
            let _ = windows::Win32::Foundation::LocalFree(Some(
                windows::Win32::Foundation::HLOCAL(argv.cast()),
            ));
            args
        }
    }

    #[test]
    fn quoted_arguments_come_back_as_they_were() {
        for arg in [
            r"C:\Users\me\My Projects",
            r"C:\",
            r"\\server\share\dir\",
            r#"odd "name""#,
            r"trailing\\",
            "plain",
            "",
        ] {
            let line = format!("NexSSH.exe --cwd {}", quote(arg));
            assert_eq!(split(&line), ["NexSSH.exe", "--cwd", arg], "{line}");
        }
        assert_eq!(quote("plain"), "plain");
        assert_eq!(quote(r"C:\"), r#""C:\\""#);
    }

    #[test]
    fn the_selected_folders_are_read_from_the_items() {
        let dir = std::env::temp_dir().join("nexssh explorer test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();
        assert_eq!(folders(Some(&items(&path))), [path]);
        // A drive's root.
        assert_eq!(folders(Some(&items(r"C:\"))), [r"C:\"]);
        assert!(folders(None).is_empty());
    }

    /// Explorer runs the program; skipped where no Explorer runs (the desktop is not there).
    #[test]
    fn explorer_runs_the_program() {
        com();
        let marker =
            std::env::temp_dir().join(format!("nexssh explorer {}.txt", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let cmd =
            std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
        let args = format!("/c echo started> {}", quote(&marker.to_string_lossy()));
        // SAFETY: COM is initialized.
        if let Err(e) = unsafe { explorer_execute(&HSTRING::from(&cmd), &args, SW_HIDE.0) } {
            eprintln!("no Explorer to ask: {e}");
            return;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !marker.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "Explorer did not run {cmd} {args}"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = std::fs::remove_file(&marker);
    }

    #[test]
    fn com_hands_out_the_command() {
        com();
        // SAFETY: the calls COM makes, with valid pointers.
        unsafe {
            let mut factory: *mut c_void = std::ptr::null_mut();
            assert_eq!(
                DllGetClassObject(&GUID::zeroed(), &IClassFactory::IID, &mut factory),
                CLASS_E_CLASSNOTAVAILABLE
            );
            assert_eq!(
                DllGetClassObject(&CLSID, &IClassFactory::IID, &mut factory),
                S_OK
            );
            let factory = IClassFactory::from_raw(factory);
            let command: IExplorerCommand = factory.CreateInstance(None).unwrap();
            assert_eq!(command.GetCanonicalName().unwrap(), CLSID);
            assert_eq!(command.GetFlags().unwrap(), ECF_DEFAULT.0 as u32);
            assert_eq!(DllCanUnloadNow(), S_FALSE);
            drop(command);
            drop(factory);
            assert_eq!(DllCanUnloadNow(), S_OK);
        }
    }
}
